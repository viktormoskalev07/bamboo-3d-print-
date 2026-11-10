# Тест сохранения: меняем всё через интерфейс -> закрываем -> открываем -> проверяем.
# Использует постоянный профиль (--profile), данные пользователя не трогает.
param([string]$Exe = "$PSScriptRoot\..\target\release\bambu-plotter.exe")
$ErrorActionPreference = "Stop"
$api = "http://127.0.0.1:7878/"
$script:fails = 0; $script:passes = 0
$profile = "persist"
$cfgDir = "$env:TEMP\bambu-plotter-profile-$profile"
$store = "$env:APPDATA\Bambu Plotter Profile $profile"

function Call($body) {
    $bytes = [Text.Encoding]::UTF8.GetBytes(($body | ConvertTo-Json -Depth 10 -Compress))
    $r = Invoke-WebRequest -UseBasicParsing -Method Post -Uri $api -Body $bytes -ContentType 'application/json; charset=utf-8' -TimeoutSec 70
    [Text.Encoding]::UTF8.GetString($r.RawContentStream.ToArray()) | ConvertFrom-Json
}
function Check($name, $cond, $info = "") {
    if ($cond) { $script:passes++; Write-Host "  OK   $name" -ForegroundColor Green }
    else { $script:fails++; Write-Host "  FAIL $name  $info" -ForegroundColor Red }
}
function Launch {
    $p = Start-Process $Exe -ArgumentList "--automation", "7878", "--offscreen", "--profile", $profile -PassThru
    for ($i = 0; $i -lt 80; $i++) { try { if ((Call @{cmd = "state" }).fonts_loaded) { return $p } } catch {}; Start-Sleep -Milliseconds 250 }
    throw "приложение не запустилось"
}
function Quit($p) {
    try { Call @{cmd = "quit" } | Out-Null } catch {}
    if (-not $p.WaitForExit(15000)) { throw "приложение не закрылось" }
}

# чистый профиль
Remove-Item -Recurse -Force $cfgDir, $store -ErrorAction SilentlyContinue

Write-Host "`n== Запуск 1: меняем настройки через интерфейс" -ForegroundColor Cyan
$p = Launch
Call @{cmd = "click"; label = "Поле текста" } | Out-Null
Call @{cmd = "key"; key = "A"; ctrl = $true } | Out-Null
Call @{cmd = "type"; text = "Сохрани меня" } | Out-Null
Call @{cmd = "click"; label = "По центру" } | Out-Null
Call @{cmd = "click"; label = "Формат бумаги" } | Out-Null
Call @{cmd = "click"; label = "A6 альбомный" } | Out-Null
Call @{cmd = "scroll"; label = "Формат бумаги"; dy = -1500 } | Out-Null
$r = Call @{cmd = "click"; label = "Показывать холостые переезды" }
Check "клик по галочке внизу панели" $r.ok $r.error
Call @{cmd = "set_config"; config = @{pen = @{z_down = 1.75; offset_x = 12.5; draw_speed = 55 }; text = @{size = 8.5 } } } | Out-Null
Call @{cmd = "click"; label = "Таблица" } | Out-Null
Call @{cmd = "click"; label = "ячейка 2:1" } | Out-Null
Call @{cmd = "key"; key = "A"; ctrl = $true } | Out-Null
Call @{cmd = "type"; text = "Апельсины" } | Out-Null
Call @{cmd = "wait"; frames = 5 } | Out-Null
$before = Call @{cmd = "state" }
Quit $p
Check "файл настроек создан" (Test-Path "$cfgDir\plotter.toml")

Write-Host "`n== Запуск 2: проверяем, что всё восстановилось" -ForegroundColor Cyan
$p = Launch
$s = Call @{cmd = "state" }
Check "текст" ($s.doc.text -eq "Сохрани меня") "text='$($s.doc.text)'"
Check "выравнивание" ($s.doc.align -eq 1) "align=$($s.doc.align)"
Check "вкладка" ($s.mode -eq "Table") "mode=$($s.mode)"
Check "ячейка таблицы" ($s.doc.table[1][0] -eq "Апельсины") "cell='$($s.doc.table[1][0])'"
Check "формат бумаги" ($s.config.paper.width -eq 148 -and $s.config.paper.height -eq 105) "paper=$($s.config.paper.width)x$($s.config.paper.height)"
Check "z_down" ($s.config.pen.z_down -eq 1.75) "z_down=$($s.config.pen.z_down)"
Check "смещение X" ($s.config.pen.offset_x -eq 12.5) "offset_x=$($s.config.pen.offset_x)"
Check "скорость" ($s.config.pen.draw_speed -eq 55) "draw_speed=$($s.config.pen.draw_speed)"
Check "размер шрифта" ($s.config.text.size -eq 8.5) "size=$($s.config.text.size)"
Check "холостые переезды (вид)" ($s.prefs.show_travel -eq $true) "show_travel=$($s.prefs.show_travel)"

Write-Host "`n== Запуск 3: аварийное завершение (kill) сразу после правки" -ForegroundColor Cyan
Call @{cmd = "set_doc"; doc = @{text = "После краха"; mode = "Text" } } | Out-Null
Call @{cmd = "set_config"; config = @{pen = @{z_down = 1.5 } } } | Out-Null
Stop-Process -Id $p.Id -Force; $p.WaitForExit(5000) | Out-Null
$p = Launch
$s = Call @{cmd = "state" }
Check "текст пережил kill" ($s.doc.text -eq "После краха") "text='$($s.doc.text)'"
Check "z_down пережил kill" ($s.config.pen.z_down -eq 1.5) "z_down=$($s.config.pen.z_down)"
Quit $p

Write-Host "`n== Запуск 4: нарисовать -> закрыть -> открыть -> щёлкнуть по рисунку -> Delete" -ForegroundColor Cyan
$p = Launch
Call @{cmd = "set_doc"; doc = @{mode = "Draw"; tool = "Pencil"; shapes = @(); text = "Текст остаётся"; show_text = $true; show_drawing = $true } } | Out-Null
$a = Call @{cmd = "area_to_screen"; x = 30; y = 120 }
$b = Call @{cmd = "area_to_screen"; x = 90; y = 140 }
Call @{cmd = "drag"; from = @{x = $a.x; y = $a.y }; to = @{x = $b.x; y = $b.y } } | Out-Null
Check "нарисовано карандашом" ((Call @{cmd = "state" }).doc.shapes.Count -eq 1)
Quit $p
$p = Launch
$s = Call @{cmd = "state" }
Check "рисунок на месте после перезапуска" ($s.doc.shapes.Count -eq 1 -and $s.doc.tool -eq "Pencil") "shapes=$($s.doc.shapes.Count) tool=$($s.doc.tool)"
$m = Call @{cmd = "area_to_screen"; x = 60; y = 130 }
Call @{cmd = "click"; x = $m.x; y = $m.y } | Out-Null
Call @{cmd = "key"; key = "Delete" } | Out-Null
$s = Call @{cmd = "state" }
Check "щелчок по рисунку + Delete удаляет его (инструмент Карандаш)" ($s.doc.shapes.Count -eq 0) "shapes=$($s.doc.shapes.Count)"
Check "текст не тронут" ($s.doc.text -eq "Текст остаётся")
Call @{cmd = "key"; key = "Z"; ctrl = $true } | Out-Null
Check "Ctrl+Z вернул" ((Call @{cmd = "state" }).doc.shapes.Count -eq 1)
# с вкладки «Текст»: щелчок по нарисованному + Delete удаляет нарисованное, а не текст
Call @{cmd = "set_doc"; doc = @{mode = "Text" } } | Out-Null
$m = Call @{cmd = "area_to_screen"; x = 60; y = 130 }
Call @{cmd = "click"; x = $m.x; y = $m.y } | Out-Null
Call @{cmd = "key"; key = "Delete" } | Out-Null
$s = Call @{cmd = "state" }
Check "с вкладки «Текст»: удалено нарисованное, текст цел" ($s.doc.shapes.Count -eq 0 -and $s.doc.text -eq "Текст остаётся") "shapes=$($s.doc.shapes.Count) text='$($s.doc.text)'"
# Delete без выделения ничего не трогает
Call @{cmd = "key"; key = "Escape" } | Out-Null
Call @{cmd = "key"; key = "Delete" } | Out-Null
Check "Delete без выделения ничего не удаляет" ((Call @{cmd = "state" }).doc.text -eq "Текст остаётся")
Quit $p

Remove-Item -Recurse -Force $cfgDir, $store -ErrorAction SilentlyContinue
Write-Host "`nИтого: $($script:passes) OK, $($script:fails) FAIL" -ForegroundColor $(if ($script:fails) { "Red" } else { "Green" })
exit $script:fails
