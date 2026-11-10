//! Отправка на Bambu Lab A1 по локальной сети:
//!  - загрузка файла по FTPS (неявный TLS, порт 990, логин bblp / код доступа)
//!  - запуск печати через MQTT (TLS, порт 8883)
//! Без облака и без Bambu Studio.
#![allow(dead_code)]

use crate::config::PrinterCfg;
use anyhow::{anyhow, bail, Context, Result};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::CryptoProvider;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, ClientConnection, DigitallySignedStruct, SignatureScheme, StreamOwned};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::time::{Duration, Instant};

type Tls = StreamOwned<ClientConnection, TcpStream>;

/// У принтера самоподписанный сертификат — проверку отключаем (только локальная сеть).
#[derive(Debug)]
struct AcceptAny(Arc<CryptoProvider>);

impl ServerCertVerifier for AcceptAny {
    fn verify_server_cert(
        &self,
        _: &CertificateDer<'_>,
        _: &[CertificateDer<'_>],
        _: &ServerName<'_>,
        _: &[u8],
        _: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        m: &[u8],
        c: &CertificateDer<'_>,
        d: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(m, c, d, &self.0.signature_verification_algorithms)
    }
    fn verify_tls13_signature(
        &self,
        m: &[u8],
        c: &CertificateDer<'_>,
        d: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(m, c, d, &self.0.signature_verification_algorithms)
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

pub struct Printer {
    cfg: PrinterCfg,
    tls: Arc<ClientConfig>,
}

impl Printer {
    pub fn new(cfg: &PrinterCfg) -> Result<Printer> {
        if cfg.ip.is_empty() || cfg.serial.is_empty() || cfg.access_code.is_empty() {
            bail!("в plotter.toml не заполнены [printer] ip / serial / access_code");
        }
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let tls = ClientConfig::builder_with_provider(provider.clone())
            .with_safe_default_protocol_versions()?
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(AcceptAny(provider)))
            .with_no_client_auth();
        Ok(Printer { cfg: cfg.clone(), tls: Arc::new(tls) })
    }

    fn connect(&self, port: u16) -> Result<Tls> {
        let addr = (self.cfg.ip.as_str(), port)
            .to_socket_addrs()?
            .next()
            .ok_or_else(|| anyhow!("неверный IP {}", self.cfg.ip))?;
        let tcp = TcpStream::connect_timeout(&addr, Duration::from_secs(5))
            .with_context(|| format!("нет связи с принтером {}:{port}", self.cfg.ip))?;
        tcp.set_read_timeout(Some(Duration::from_secs(20)))?;
        tcp.set_write_timeout(Some(Duration::from_secs(20)))?;
        let name = ServerName::try_from(self.cfg.ip.clone())?;
        let conn = ClientConnection::new(self.tls.clone(), name)?;
        Ok(StreamOwned::new(conn, tcp))
    }

    // ---------------- FTPS ----------------

    pub fn upload(&self, remote_name: &str, data: &[u8]) -> Result<()> {
        let mut ctrl = BufReader::new(self.connect(990)?);
        expect(&mut ctrl, None, 220)?;
        expect(&mut ctrl, Some("USER bblp"), 331)?;
        expect(&mut ctrl, Some(&format!("PASS {}", self.cfg.access_code)), 230)
            .context("принтер не принял код доступа")?;
        expect(&mut ctrl, Some("PBSZ 0"), 200)?;
        expect(&mut ctrl, Some("PROT P"), 200)?;
        expect(&mut ctrl, Some("TYPE I"), 200)?;
        let (_, pasv) = expect(&mut ctrl, Some("PASV"), 227)?;
        let port = parse_pasv(&pasv)?;

        let addr = (self.cfg.ip.as_str(), port).to_socket_addrs()?.next().unwrap();
        let tcp = TcpStream::connect_timeout(&addr, Duration::from_secs(5))?;
        tcp.set_write_timeout(Some(Duration::from_secs(60)))?;
        tcp.set_read_timeout(Some(Duration::from_secs(60)))?;
        send(&mut ctrl, &format!("STOR {remote_name}"))?;
        let (code, msg) = reply(&mut ctrl)?;
        if code != 150 && code != 125 {
            bail!("FTP STOR: {code} {msg}");
        }
        // TLS на канале данных (сессия переиспользуется — общий ClientConfig)
        let name = ServerName::try_from(self.cfg.ip.clone())?;
        let mut data_conn = StreamOwned::new(ClientConnection::new(self.tls.clone(), name)?, tcp);
        let total = data.len();
        let mut sent = 0;
        for chunk in data.chunks(64 * 1024) {
            data_conn.write_all(chunk)?;
            sent += chunk.len();
            eprint!("\r  загрузка: {}%   ", sent * 100 / total.max(1));
        }
        eprintln!();
        data_conn.flush()?;
        data_conn.conn.send_close_notify();
        while data_conn.conn.wants_write() {
            data_conn.conn.write_tls(&mut data_conn.sock)?;
        }
        let _ = data_conn.sock.shutdown(std::net::Shutdown::Write);
        drop(data_conn);
        let (code, msg) = reply(&mut ctrl)?;
        if code != 226 {
            bail!("FTP: передача не завершилась: {code} {msg}");
        }
        let _ = send(&mut ctrl, "QUIT");
        Ok(())
    }

    // ---------------- MQTT ----------------

    fn mqtt(&self) -> Result<Tls> {
        let mut s = self.connect(8883)?;
        let mut p = Vec::new();
        put_str(&mut p, "MQTT");
        p.push(4); // MQTT 3.1.1
        p.push(0xC2); // username + password + clean session
        p.extend_from_slice(&60u16.to_be_bytes());
        put_str(&mut p, &format!("bambu-plotter-{}", std::process::id()));
        put_str(&mut p, "bblp");
        put_str(&mut p, &self.cfg.access_code);
        write_packet(&mut s, 0x10, &p)?;
        let (t, body) = read_packet(&mut s)?;
        if t >> 4 != 2 || body.len() < 2 {
            bail!("MQTT: неожиданный ответ на CONNECT");
        }
        match body[1] {
            0 => {}
            4 | 5 => bail!("MQTT: неверный код доступа (или выключен LAN/режим разработчика)"),
            c => bail!("MQTT: принтер отказал в подключении, код {c}"),
        }
        // подписка на отчёты
        let mut sub = vec![0, 1];
        put_str(&mut sub, &format!("device/{}/report", self.cfg.serial));
        sub.push(0);
        write_packet(&mut s, 0x82, &sub)?;
        Ok(s)
    }

    fn publish(&self, s: &mut Tls, payload: &serde_json::Value) -> Result<()> {
        let mut p = Vec::new();
        put_str(&mut p, &format!("device/{}/request", self.cfg.serial));
        p.extend_from_slice(payload.to_string().as_bytes());
        write_packet(s, 0x30, &p)
    }

    /// Читает отчёты принтера до `timeout`, вызывая `f` для каждого. `f` возвращает true — хватит.
    fn listen(&self, s: &mut Tls, timeout: Duration, mut f: impl FnMut(&serde_json::Value) -> bool) -> Result<()> {
        s.sock.set_read_timeout(Some(Duration::from_millis(500)))?;
        let end = Instant::now() + timeout;
        while Instant::now() < end {
            match read_packet(s) {
                Ok((t, body)) if t >> 4 == 3 && body.len() > 2 => {
                    let tl = u16::from_be_bytes([body[0], body[1]]) as usize;
                    let qos = (t >> 1) & 3;
                    let start = 2 + tl + if qos > 0 { 2 } else { 0 };
                    if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&body[start.min(body.len())..]) {
                        if f(&v) {
                            return Ok(());
                        }
                    }
                }
                Ok(_) => {}
                Err(e) if is_timeout(&e) => {}
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    pub fn start_print(&self, remote_name: &str) -> Result<()> {
        let mut s = self.mqtt()?;
        let cmd = serde_json::json!({
            "print": {
                "sequence_id": "1",
                "command": "project_file",
                "param": "Metadata/plate_1.gcode",
                "project_id": "0",
                "profile_id": "0",
                "task_id": "0",
                "subtask_id": "0",
                "subtask_name": remote_name,
                "file": remote_name,
                "url": format!("file:///sdcard/{remote_name}"),
                "md5": "",
                "timelapse": false,
                "bed_type": "auto",
                "bed_levelling": false,
                "flow_cali": false,
                "vibration_cali": false,
                "layer_inspect": false,
                "use_ams": false,
                "ams_mapping": [],
            }
        });
        self.publish(&mut s, &cmd)?;
        eprintln!("Команда печати отправлена, жду ответ принтера...");
        let mut last = String::new();
        let mut ok = false;
        self.listen(&mut s, Duration::from_secs(20), |v| {
            let p = &v["print"];
            if p["command"] == "project_file" {
                if let Some(r) = p["result"].as_str() {
                    eprintln!("  принтер: {r} {}", p["reason"].as_str().unwrap_or(""));
                }
            }
            if let Some(st) = p["gcode_state"].as_str() {
                if st != last {
                    eprintln!("  состояние: {st}");
                    last = st.to_string();
                }
                if st == "PREPARE" || st == "RUNNING" {
                    ok = true;
                    return true;
                }
                if st == "FAILED" {
                    return true;
                }
            }
            if let Some(e) = p["print_error"].as_i64() {
                if e != 0 {
                    eprintln!("  ошибка принтера: {e:#X}");
                }
            }
            false
        })?;
        if !ok {
            eprintln!("Принтер не подтвердил старт. Проверьте экран принтера.");
        }
        Ok(())
    }

    pub fn status(&self) -> Result<()> {
        let mut s = self.mqtt()?;
        self.publish(&mut s, &serde_json::json!({"pushing": {"sequence_id": "0", "command": "pushall"}}))?;
        let mut got = false;
        self.listen(&mut s, Duration::from_secs(10), |v| {
            let p = &v["print"];
            if p["gcode_state"].is_string() {
                println!("Состояние:   {}", p["gcode_state"].as_str().unwrap_or("?"));
                println!("Файл:        {}", p["gcode_file"].as_str().unwrap_or("-"));
                println!("Прогресс:    {}%", p["mc_percent"].as_i64().unwrap_or(0));
                println!("Осталось:    {} мин", p["mc_remaining_time"].as_i64().unwrap_or(0));
                println!("Сопло:       {} °C", p["nozzle_temper"].as_f64().unwrap_or(0.0));
                got = true;
                return true;
            }
            false
        })?;
        if !got {
            bail!("подключился, но принтер не прислал статус (проверьте серийный номер)");
        }
        Ok(())
    }

    pub fn stop(&self) -> Result<()> {
        let mut s = self.mqtt()?;
        self.publish(&mut s, &serde_json::json!({"print": {"sequence_id": "0", "command": "stop", "param": ""}}))?;
        std::thread::sleep(Duration::from_millis(500));
        Ok(())
    }
}

fn is_timeout(e: &anyhow::Error) -> bool {
    e.downcast_ref::<std::io::Error>()
        .is_some_and(|e| matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut))
}

fn send(ctrl: &mut BufReader<Tls>, cmd: &str) -> Result<()> {
    ctrl.get_mut().write_all(format!("{cmd}\r\n").as_bytes())?;
    ctrl.get_mut().flush()?;
    Ok(())
}

fn reply(ctrl: &mut BufReader<Tls>) -> Result<(u32, String)> {
    let mut full = String::new();
    loop {
        let mut line = String::new();
        if ctrl.read_line(&mut line)? == 0 {
            bail!("FTP: соединение закрыто");
        }
        full.push_str(&line);
        let b = line.as_bytes();
        if b.len() >= 4 && b[..3].iter().all(u8::is_ascii_digit) && b[3] == b' ' {
            return Ok((line[..3].parse()?, full.trim().to_string()));
        }
    }
}

fn expect(ctrl: &mut BufReader<Tls>, cmd: Option<&str>, code: u32) -> Result<(u32, String)> {
    if let Some(c) = cmd {
        send(ctrl, c)?;
    }
    let (got, msg) = reply(ctrl)?;
    if got != code {
        let shown = cmd.map(|c| if c.starts_with("PASS") { "PASS ****" } else { c }).unwrap_or("подключение");
        bail!("FTP {shown}: ожидался {code}, получено: {msg}");
    }
    Ok((got, msg))
}

fn parse_pasv(msg: &str) -> Result<u16> {
    let inner = msg.split('(').nth(1).and_then(|s| s.split(')').next()).ok_or_else(|| anyhow!("PASV: {msg}"))?;
    let n: Vec<u16> = inner.split(',').filter_map(|x| x.trim().parse().ok()).collect();
    if n.len() != 6 {
        bail!("PASV: {msg}");
    }
    Ok(n[4] * 256 + n[5])
}

fn put_str(buf: &mut Vec<u8>, s: &str) {
    buf.extend_from_slice(&(s.len() as u16).to_be_bytes());
    buf.extend_from_slice(s.as_bytes());
}

fn write_packet(s: &mut Tls, kind: u8, body: &[u8]) -> Result<()> {
    let mut p = vec![kind];
    let mut len = body.len();
    loop {
        let mut b = (len % 128) as u8;
        len /= 128;
        if len > 0 {
            b |= 0x80;
        }
        p.push(b);
        if len == 0 {
            break;
        }
    }
    p.extend_from_slice(body);
    s.write_all(&p)?;
    s.flush()?;
    Ok(())
}

fn read_packet(s: &mut Tls) -> Result<(u8, Vec<u8>)> {
    let mut b = [0u8; 1];
    s.read_exact(&mut b)?;
    let kind = b[0];
    let (mut len, mut mul) = (0usize, 1usize);
    loop {
        s.read_exact(&mut b)?;
        len += (b[0] & 0x7F) as usize * mul;
        mul *= 128;
        if b[0] & 0x80 == 0 {
            break;
        }
    }
    let mut body = vec![0u8; len];
    // после первого байта читаем пакет целиком с нормальным таймаутом
    s.sock.set_read_timeout(Some(Duration::from_secs(10)))?;
    let r = s.read_exact(&mut body);
    s.sock.set_read_timeout(Some(Duration::from_millis(500)))?;
    r?;
    Ok((kind, body))
}
