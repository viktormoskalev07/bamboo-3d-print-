# Автотест интерфейса Bambu Plotter через API автоматизации.
# Запуск:  powershell -File tests\ui_test.ps1 [-Exe path] [-Shots dir]
param(
    [string]$Exe = "$PSScriptRoot\..\target\release\bambu-plotter.exe",
    [string]$Shots = "$env:TEMP\bambu-plotter-test\shots"
)
$ErrorActionPreference = "Stop"
$api = "http://127.0.0.1:7878/"
$script:fails = 0; $script:passes = 0
New-Item -ItemType Directory -Force $Shots | Out-Null

function Call($body) {
    $json = $body | ConvertTo-Json -Depth 10 -Compress
    $bytes = [Text.Encoding]::UTF8.GetBytes($json)
    $r = Invoke-WebRequest -UseBasicParsing -Method Post -Uri $api -Body $bytes -ContentType 'application/json; charset=utf-8' -TimeoutSec 70
    $text = [Text.Encoding]::UTF8.GetString($r.RawContentStream.ToArray())
    return $text | ConvertFrom-Json
}
function State { Call @{cmd = "state" } }
function Click($label, $index = 0) {
    $r = Call @{cmd = "click"; label = $label; index = $index }
    if (-not $r.ok) { throw "click '$label': $($r.error)" }
}
function Shot($name) { $r = Call @{cmd = "screenshot"; path = "$Shots\$name.png" }; if (-not $r.ok) { throw "screenshot: $($r.error)" } }
function Check($name, $cond, $info = "") {
    if ($cond) { $script:passes++; Write-Host "  OK   $name" -ForegroundColor Green }
    else { $script:fails++; Write-Host "  FAIL $name  $info" -ForegroundColor Red }
}
function Test($title, [scriptblock]$body) {
    Write-Host "`n== $title" -ForegroundColor Cyan
    try { & $body } catch { $script:fails++; Write-Host "  FAIL исключение: $_" -ForegroundColor Red }
}

# --- запуск ---
$running = $false
try { State | Out-Null; $running = $true } catch {}
if (-not $running) {
    Start-Process $Exe -ArgumentList "--automation", "7878", "--offscreen" | Out-Null
    for ($i = 0; $i -lt 80; $i++) { try { if ((State).fonts_loaded) { break } } catch {}; Start-Sleep -Milliseconds 250 }
}
Call @{cmd = "set_doc"; doc = @{mode = "Text"; text = "Привет"; align = 0; table_header = $true; show_text = $true; show_table = $false; show_image = $false; show_drawing = $true; shapes = @(); text_pos = @(0, 0); table_pos = $null; svg_pos = $null;
        table = @(@("Товар", "Кол-во", "Цена"), @("Яблоки", "3", "120,50"), @("Груши", "12", "99")) } } | Out-Null
Call @{cmd = "set_config"; config = @{paper = @{width = 148; height = 210; margin = 10; origin_x = $null; origin_y = $null }; pen = @{z_down = 3.0; offset_x = 0; offset_y = 0 } } } | Out-Null

Test "Вкладки" {
    foreach ($t in @(@("Таблица", "Table"), @("Рисунок", "Image"), @("Калибровка", "Calibrate"), @("Текст", "Text"))) {
        Click $t[0]; Check "вкладка $($t[0])" ((State).mode -eq $t[1]) "mode=$((State).mode)"
    }
}

Test "Ввод текста с клавиатуры" {
    Click "Поле текста"          # поле редактора (index 1: первое совпадение — вкладка)
    Call @{cmd = "key"; key = "A"; ctrl = $true } | Out-Null
    Call @{cmd = "type"; text = "Тест плоттера 123" } | Out-Null
    $s = State
    Check "текст введён" ($s.doc.text -eq "Тест плоттера 123") "text='$($s.doc.text)'"
    Check "превью перестроено" ($s.pages -eq 1 -and $s.page_stats[0].strokes -gt 10) "strokes=$($s.page_stats[0].strokes)"
    Call @{cmd = "key"; key = "Enter" } | Out-Null
    Call @{cmd = "type"; text = "вторая строка" } | Out-Null
    Check "перенос строки Enter" ((State).doc.text -eq "Тест плоттера 123`nвторая строка") "text='$((State).doc.text)'"
    Shot "01_text"
}

Test "Выравнивание" {
    Click "По центру"; Check "по центру" ((State).doc.align -eq 1)
    Click "Вправо"; Check "вправо" ((State).doc.align -eq 2)
    Click "Влево"; Check "влево" ((State).doc.align -eq 0)
}

Test "Многостраничный текст" {
    $long = (1..120 | ForEach-Object { "Строка номер $_" }) -join "`n"
    Call @{cmd = "set_doc"; doc = @{text = $long } } | Out-Null
    $s = State
    Check "несколько страниц" ($s.pages -ge 2) "pages=$($s.pages)"
    Click "Следующая страница"
    Check "переход на стр. 2" ((State).page -eq 2) "page=$((State).page)"
    Click "Предыдущая страница"
    Check "назад на стр. 1" ((State).page -eq 1)
    Shot "02_pages"
    Call @{cmd = "set_doc"; doc = @{text = "Привет" } } | Out-Null
}

Test "Таблица" {
    Click "Таблица"
    Click "+ Строка"
    $s = State
    Check "добавлена строка" ($s.doc.table.Count -eq 4) "rows=$($s.doc.table.Count)"
    Click "ячейка 4:1"
    Call @{cmd = "type"; text = "Сливы" } | Out-Null
    Click "ячейка 4:2"
    Call @{cmd = "type"; text = "7" } | Out-Null
    $s = State
    Check "ввод в ячейку" ($s.doc.table[3][0] -eq "Сливы" -and $s.doc.table[3][1] -eq "7") "row4=$($s.doc.table[3] -join '|')"
    Click "+ Столбец"; Check "добавлен столбец" ((State).doc.table[0].Count -eq 4)
    Click "− Столбец"; Check "удалён столбец" ((State).doc.table[0].Count -eq 3)
    Click "Первая строка — заголовок"
    Check "чекбокс заголовка" ((State).doc.table_header -eq $false)
    Click "Первая строка — заголовок"
    Check "чекбокс обратно" ((State).doc.table_header -eq $true)
    Check "таблица нарисована" ((State).page_stats[0].strokes -gt 20)
    Shot "03_table"
}

Test "Формат бумаги (выпадающий список)" {
    Click "Формат бумаги"
    Click "A6 альбомный"
    $p = (State).config.paper
    Check "A6 альбомный" ($p.width -eq 148 -and $p.height -eq 105) "paper=$($p.width)x$($p.height)"
    Shot "04_a6"
    Click "Формат бумаги"; Click "A5 книжный"
    Check "A5 обратно" ((State).config.paper.height -eq 210)
}

Test "Выход за стол" {
    Call @{cmd = "set_config"; config = @{paper = @{origin_x = 200; origin_y = 30 } } } | Out-Null
    $s = State
    Check "текст сам ужимается в зону ручки" ($null -eq $s.page_stats[0].error -and $s.usable_area[2] -lt 60) "usable=$($s.usable_area -join ',') err=$($s.page_stats[0].error)"
    Call @{cmd = "set_doc"; doc = @{show_drawing = $true; shapes = @(@{Line = @(@(0, 5), @(140, 5)) }) } } | Out-Null
    $s = State
    Check "фигура за краем — не ошибка, а предупреждение" ($null -eq $s.page_stats[0].error -and $null -ne $s.page_stats[0].warning) "err=$($s.page_stats[0].error) warn=$($s.page_stats[0].warning)"
    $e = Call @{cmd = "export" }
    Check "экспорт разрешён, всё в пределах стола" ($e.ok -and $e.nozzle_x[1] -le 256) "ok=$($e.ok) x=$($e.nozzle_x -join '..') $($e.error)"
    Shot "05_clipped"
    Call @{cmd = "set_doc"; doc = @{shapes = @() } } | Out-Null
    Call @{cmd = "set_config"; config = @{paper = @{origin_x = $null; origin_y = $null } } } | Out-Null
    Check "предупреждение ушло" ($null -eq (State).page_stats[0].warning)
}

Test "Калибровка" {
    Click "Калибровка"
    Click "выбрать" 2
    $z = (State).config.pen.z_down
    Check "z_down выбран из списка" ([math]::Abs($z - 3.5) -lt 0.001) "z_down=$z"
    Shot "06_calibrate"
    Call @{cmd = "set_config"; config = @{pen = @{z_down = 3.0 } } } | Out-Null
}

Test "SVG" {
    $svg = "$env:TEMP\bambu-plotter-test\test.svg"
    '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 60"><rect x="5" y="5" width="90" height="50" rx="8" fill="none" stroke="black"/><circle cx="30" cy="30" r="15" fill="none" stroke="black"/><path d="M50 45 C60 10 80 10 90 45" fill="none" stroke="black"/></svg>' | Set-Content $svg -Encoding utf8
    $r = Call @{cmd = "open_file"; path = $svg }
    $s = State
    Check "SVG открыт" ($s.mode -eq "Image" -and $s.pages -eq 1 -and $s.page_stats[0].strokes -ge 3) "mode=$($s.mode) strokes=$($s.page_stats[0].strokes)"
    Check "имя задания из файла" ($s.job_name -eq "test")
    Click "Повернуть вправо"; Check "поворот" ((State).doc.svg_rot -eq 1)
    Shot "07_svg_rot"
    Click "Повернуть влево"
}

Test "Превью: масштаб, симуляция" {
    Click "Текст"
    $z0 = (State).zoom
    Call @{cmd = "scroll"; label = "Превью"; dy = 400 } | Out-Null
    Check "колесо меняет масштаб" ((State).zoom -gt $z0 * 1.2) "zoom=$((State).zoom)"
    Click "Вписать"; Check "Вписать сбрасывает" ([math]::Abs((State).zoom - 1) -lt 0.001)
    Call @{cmd = "sim"; value = 0.4 } | Out-Null
    Shot "08_sim40"
    Click "Симуляция"
    Call @{cmd = "wait"; frames = 20 } | Out-Null
    $s1 = (State).sim
    Check "анимация идёт" ($s1 -gt 0.4 -and $s1 -le 1) "sim=$s1"
    Call @{cmd = "sim"; value = 1.0 } | Out-Null
}

Test "Экспорт .gcode.3mf" {
    Call @{cmd = "set_doc"; doc = @{text = "Привет, мир!" } } | Out-Null
    $e = Call @{cmd = "export"; path = "$env:TEMP\bambu-plotter-test\t.gcode.3mf" }
    Check "экспорт ok" $e.ok $e.error
    Check "состав 3mf" (($e.entries -contains "Metadata/plate_1.gcode") -and ($e.entries -contains "Metadata/slice_info.config") -and ($e.entries -contains "Metadata/plate_1.png"))
    Check "md5 совпадает" $e.md5_ok
    Check "нет нагрева" (-not $e.heating)
    Check "нет экструзии" (-not $e.extrusion)
    Check "только ASCII" (-not $e.non_ascii)
    Check "есть G28" $e.has_g28
    Check "X в пределах стола" ($e.nozzle_x[0] -ge 0 -and $e.nozzle_x[1] -le 256) "x=$($e.nozzle_x -join '..')"
    Check "Y в пределах стола" ($e.nozzle_y[0] -ge 0 -and $e.nozzle_y[1] -le 256) "y=$($e.nozzle_y -join '..')"
    Check "Z: z_down..z_up+10" ($e.z[0] -ge 2.99 -and $e.z[1] -le 18.01) "z=$($e.z -join '..')"
}

Test "Смещение ручки" {
    Call @{cmd = "set_config"; config = @{paper = @{origin_x = 54; origin_y = 23 } } } | Out-Null
    Call @{cmd = "set_config"; config = @{pen = @{offset_x = 20; offset_y = -10 } } } | Out-Null
    $a = Call @{cmd = "export"; path = "$env:TEMP\bambu-plotter-test\off.gcode.3mf" }
    Call @{cmd = "set_config"; config = @{pen = @{offset_x = 0; offset_y = 0 } } } | Out-Null
    $b = Call @{cmd = "export"; path = "$env:TEMP\bambu-plotter-test\nooff.gcode.3mf" }
    Check "сопло сдвинуто на -offset_x" ([math]::Abs(($b.nozzle_x[1] - $a.nozzle_x[1]) - 20) -lt 0.01) "dx=$($b.nozzle_x[1] - $a.nozzle_x[1])"
    Check "сопло сдвинуто на -offset_y" ([math]::Abs(($a.nozzle_y[0] - $b.nozzle_y[0]) - 10) -lt 0.01) "dy=$($a.nozzle_y[0] - $b.nozzle_y[0])"
    Call @{cmd = "set_config"; config = @{paper = @{origin_x = $null; origin_y = $null } } } | Out-Null
    Call @{cmd = "set_config"; config = @{pen = @{offset_y = -40 } } } | Out-Null
    $r = State
    Check "лист центрируется в зоне ручки" ($null -eq $r.page_stats[0].error) "error=$($r.page_stats[0].error)"
    Call @{cmd = "set_config"; config = @{pen = @{offset_y = 0 } } } | Out-Null
}

Test "Масштаб: реальные размеры в мм" {
    Call @{cmd = "set_config"; config = @{pen = @{offset_x = 0; offset_y = 0 }; text = @{font = ""; size = 10 } } } | Out-Null
    Call @{cmd = "set_doc"; doc = @{mode = "Draw"; show_text = $false; show_table = $false; show_image = $false; show_drawing = $true; shapes = @(@{Grid = @{a = @(8, 8); b = @(108, 108); step = 10; labels = $false } }) } } | Out-Null
    $e = Call @{cmd = "export" }
    $w = $e.bbox[2] - $e.bbox[0]; $h = $e.bbox[3] - $e.bbox[1]
    Check "сетка 100×100 мм" ([math]::Abs($w - 100) -lt 0.01 -and [math]::Abs($h - 100) -lt 0.01) "bbox=$($w)x$($h)"
    Call @{cmd = "set_doc"; doc = @{mode = "Text"; text = "H"; show_text = $true; show_drawing = $false } } | Out-Null
    $e = Call @{cmd = "export" }
    $h = $e.bbox[3] - $e.bbox[1]
    Check "буква H высотой 10 мм" ([math]::Abs($h - 10) -lt 0.05) "h=$h"
    Call @{cmd = "set_config"; config = @{text = @{size = 6 } } } | Out-Null
}

Test "Порядок букв" {
    Call @{cmd = "set_doc"; doc = @{mode = "Text"; text = "ШКОЛА МИР"; align = 0; show_text = $true; show_table = $false; show_image = $false; show_drawing = $false } } | Out-Null
    $e = Call @{cmd = "export"; path = "$env:TEMP\bambu-plotter-test\order.gcode.3mf" }
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $z = [IO.Compression.ZipFile]::OpenRead("$env:TEMP\bambu-plotter-test\order.gcode.3mf")
    $sr = New-Object IO.StreamReader(($z.Entries | Where-Object FullName -eq "Metadata/plate_1.gcode").Open())
    $lines = $sr.ReadToEnd() -split "`n"; $z.Dispose()
    # X каждого опускания ручки (первая точка штриха) должен в основном расти слева направо
    $xs = @(); $prevZ = 99
    foreach ($l in $lines) {
        if ($l -match '^G1 Z([\d.]+)') { $zv = [double]$Matches[1]; if ($zv -lt 5 -and $prevZ -ge 5) { $xs += $lastX }; $prevZ = $zv }
        elseif ($l -match '^G1 X([\d.]+)') { $lastX = [double]$Matches[1] }
    }
    $back = 0; for ($i = 1; $i -lt $xs.Count; $i++) { if ($xs[$i] -lt $xs[$i - 1] - 6) { $back++ } }
    Check "буквы по порядку слева направо" ($back -eq 0) "возвратов назад: $back из $($xs.Count) штрихов"
}

Test "Превью = G-code" {
    Call @{cmd = "set_config"; config = @{pen = @{offset_x = 0; offset_y = 0; invert_x = $false; invert_y = $false } } } | Out-Null
    Call @{cmd = "set_doc"; doc = @{mode = "Text"; text = "Привет, мир!"; show_table = $false; show_image = $false; show_drawing = $false } } | Out-Null
    $s = State; $p = $s.page_stats[0]
    Check "длина линий превью = длина в G-code" ([math]::Abs($p.preview_draw_m - $p.draw_m) -lt 0.011) "preview=$($p.preview_draw_m) gcode=$($p.draw_m)"
    $a = Call @{cmd = "export" }
    $bbA = $p.preview_bbox
    Call @{cmd = "set_config"; config = @{pen = @{invert_y = $true } } } | Out-Null
    $s2 = State; $b = Call @{cmd = "export" }
    Check "отражение Y: превью не меняется" (($s2.page_stats[0].preview_bbox -join ',') -eq ($bbA -join ',')) "$($s2.page_stats[0].preview_bbox -join ',') vs $($bbA -join ',')"
    Check "отражение Y: G-code перевёрнут" ($a.nozzle_y[0] -gt 150 -and $b.nozzle_y[0] -lt 100) "Ymin обычн=$($a.nozzle_y[0]) отраж=$($b.nozzle_y[0])"
    Call @{cmd = "set_config"; config = @{pen = @{invert_y = $false } } } | Out-Null
}

Test "Тест направления" {
    Click "Калибровка"
    Call @{cmd = "scroll"; label = "Название задания"; dy = -1500 } | Out-Null
    Click "Открыть тест направления"
    $s = State
    Check "тест направления открыт" ($s.mode -eq "Draw" -and $s.doc.shapes.Count -ge 10 -and $null -eq $s.page_stats[0].error) "shapes=$($s.doc.shapes.Count) err=$($s.page_stats[0].error)"
    Shot "11_orientation"
}

Test "Рисование мышью" {
    Click "Рисование"
    Check "вкладка Рисование" ((State).mode -eq "Draw")
    Click "Очистить лист"
    Click "Прямоугольник"
    $c = (Call @{cmd = "widgets" }).widgets | Where-Object label -eq "Превью"
    Call @{cmd = "drag"; from = @{x = $c.x - 60; y = $c.y - 60 }; to = @{x = $c.x + 40; y = $c.y + 20 } } | Out-Null
    $s = State
    Check "нарисован прямоугольник" ($s.doc.shapes.Count -eq 1 -and $null -ne $s.doc.shapes[0].Rect) "shapes=$($s.doc.shapes | ConvertTo-Json -Compress -Depth 5)"
    $r = $s.doc.shapes[0].Rect
    Check "привязка к 1 мм" (($r[0][0] % 1) -eq 0 -and ($r[1][1] % 1) -eq 0) "rect=$($r | ConvertTo-Json -Compress)"
    Click "Круг"
    Call @{cmd = "drag"; from = @{x = $c.x; y = $c.y + 80 }; to = @{x = $c.x + 30; y = $c.y + 80 } } | Out-Null
    Check "нарисован круг" ((State).doc.shapes.Count -eq 2)
    Click "Отменить"
    Check "отмена" ((State).doc.shapes.Count -eq 1)
    Click "Тест масштаба"
    $s = State
    Check "тест масштаба добавлен" ($s.doc.shapes.Count -eq 2 -and $s.page_stats[0].strokes -gt 20)
    Shot "10_draw"
    Click "Очистить лист"
}

Test "Единый лист: всё вместе" {
    Call @{cmd = "set_config"; config = @{paper = @{width = 148; height = 210; origin_x = $null; origin_y = $null }; pen = @{offset_x = 0; offset_y = 0 }; text = @{size = 5 } } } | Out-Null
    $svg = "$env:TEMPambu-plotter-test	est.svg"
    Call @{cmd = "set_doc"; doc = @{mode = "Text"; text = "Заголовок листа"; show_text = $true; show_table = $true; show_drawing = $true; show_image = $true;
            table = @(@("A", "B"), @("1", "2")); text_pos = @(0, 0); table_pos = $null; svg_pos = $null; svg_fit = $false; svg_scale = 50;
            shapes = @(@{Rect = @(@(80, 150), @(120, 180)) }) } } | Out-Null
    Call @{cmd = "open_file"; path = $svg } | Out-Null
    $s = State
    $elems = @($s.page_stats[0].boxes | ForEach-Object { $_.elem })
    Check "на одном листе текст+таблица+SVG+рисование" (($elems -contains "Text") -and ($elems -contains "Table") -and ($elems -contains "Image") -and ($elems -contains "Drawing")) "elems=$($elems -join ',')"
    $tb = ($s.page_stats[0].boxes | Where-Object elem -eq "Text").bbox
    $tab = ($s.page_stats[0].boxes | Where-Object elem -eq "Table").bbox
    Check "таблица под текстом" ($tab[1] -gt $tb[3]) "text bottom=$($tb[3]) table top=$($tab[1])"
    Check "нет ошибок" ($null -eq $s.page_stats[0].error) "$($s.page_stats[0].error)"
    Shot "12_all_in_one"
    # перетаскивание текста мышью
    Click "Текст"
    $c = (Call @{cmd = "widgets" }).widgets | Where-Object label -eq "Превью"
    $w = (State).zoom
    # центр текстового блока на экране: берём из виджета превью и bbox через пробный клик-драг
    $before = (State).doc.text_pos
    $el = (Call @{cmd = "widgets" }).widgets | Where-Object label -eq "элемент: Текст"
    $r = Call @{cmd = "drag"; from = @{x = $el.x; y = $el.y }; to = @{x = $el.x + 40; y = $el.y + 30 } }
    $after = (State).doc.text_pos
    Check "текст перетаскивается мышью" (($after[0] -ne $before[0]) -or ($after[1] -ne $before[1])) "до=$($before -join ',') после=$($after -join ',')"
    Shot "13_moved"
    Call @{cmd = "set_doc"; doc = @{text_pos = @(0, 0); show_table = $false; show_image = $false; shapes = @() } } | Out-Null
}

Test "A4" {
    Call @{cmd = "click"; label = "Формат бумаги" } | Out-Null
    $r = Call @{cmd = "click"; label = "A4 книжный" }
    $s = State
    Check "A4 выбран" ($s.config.paper.width -eq 210 -and $s.config.paper.height -eq 297) "paper=$($s.config.paper.width)x$($s.config.paper.height) $($r.error)"
    Call @{cmd = "set_doc"; doc = @{mode = "Text"; show_text = $true; text = ((1..60 | ForEach-Object { "Строка $_ на листе A4" }) -join "`n") } } | Out-Null
    $s = State
    $u = $s.usable_area
    Check "раскладка только в зоне ручки" ($null -eq $s.page_stats[0].error -and $u[3] -le 277) "usable=$($u -join ',') err=$($s.page_stats[0].error)"
    Shot "14_a4"
    Call @{cmd = "set_config"; config = @{paper = @{width = 148; height = 210 }; text = @{size = 6 } } } | Out-Null
}

Test "Горячие клавиши и удаление" {
    Call @{cmd = "set_config"; config = @{paper = @{width = 148; height = 210; origin_x = $null; origin_y = $null }; pen = @{offset_x = 0; offset_y = 0; scale_x = 1; scale_y = 1 } } } | Out-Null
    Call @{cmd = "set_doc"; doc = @{mode = "Draw"; show_text = $false; show_table = $false; show_image = $false; show_drawing = $true; tool = "Select"; text_pos = @(0, 0);
            shapes = @(@{Rect = @(@(20, 20), @(60, 50)) }, @{Circle = @(@(90, 120), 15) }) } } | Out-Null
    Call @{cmd = "key"; key = "Escape" } | Out-Null
    # выбрать прямоугольник щелчком по его верхней стороне (40, 20) мм
    $pt = Call @{cmd = "area_to_screen"; x = 40; y = 20 }
    Call @{cmd = "click"; x = $pt.x; y = $pt.y } | Out-Null
    Call @{cmd = "key"; key = "Delete" } | Out-Null
    $s = State
    Check "Delete удаляет выбранную фигуру" ($s.doc.shapes.Count -eq 1 -and $null -ne $s.doc.shapes[0].Circle) "shapes=$($s.doc.shapes | ConvertTo-Json -Compress -Depth 5)"
    Call @{cmd = "key"; key = "Z"; ctrl = $true } | Out-Null
    Check "Ctrl+Z возвращает" ((State).doc.shapes.Count -eq 2) "n=$((State).doc.shapes.Count)"
    Call @{cmd = "key"; key = "Y"; ctrl = $true } | Out-Null
    Check "Ctrl+Y повторяет удаление" ((State).doc.shapes.Count -eq 1) "n=$((State).doc.shapes.Count)"
    # выбрать круг (правый край: 105, 120 мм), дублировать, сдвинуть стрелкой
    Call @{cmd = "key"; key = "V" } | Out-Null
    $pt = Call @{cmd = "area_to_screen"; x = 105; y = 120 }
    Call @{cmd = "click"; x = $pt.x; y = $pt.y } | Out-Null
    Call @{cmd = "key"; key = "D"; ctrl = $true } | Out-Null
    $s = State
    Check "Ctrl+D дублирует" ($s.doc.shapes.Count -eq 2) "n=$($s.doc.shapes.Count)"
    $before = $s.doc.shapes[1].Circle[0][0]
    Call @{cmd = "key"; key = "ArrowRight"; shift = $true } | Out-Null
    $after = (State).doc.shapes[1].Circle[0][0]
    Check "Shift+стрелка сдвигает на 10 мм" ([math]::Abs($after - $before - 10) -lt 0.01) "x: $before -> $after"
    Call @{cmd = "key"; key = "L" } | Out-Null
    Check "клавиша L — инструмент «Линия»" ((State).doc.tool -eq "Line")
    Call @{cmd = "key"; key = "2"; ctrl = $true } | Out-Null
    Check "Ctrl+2 — вкладка «Таблица»" ((State).mode -eq "Table") "mode=$((State).mode)"
    # стрелки двигают текст во вкладке «Текст»
    Call @{cmd = "set_doc"; doc = @{mode = "Text"; show_text = $true; text = "Сдвиг"; text_pos = @(0, 0) } } | Out-Null
    Call @{cmd = "key"; key = "ArrowRight" } | Out-Null
    Call @{cmd = "key"; key = "ArrowDown" } | Out-Null
    $tp = (State).doc.text_pos
    Check "стрелки двигают текст" ($tp[0] -eq 1 -and $tp[1] -eq 1) "text_pos=$($tp -join ',')"
}

Test "Очистить всё" {
    Call @{cmd = "set_doc"; doc = @{text = "что-то"; shapes = @(@{Line = @(@(0, 0), @(10, 10)) }); show_drawing = $true } } | Out-Null
    Click "Очистить всё"
    Click "Да, очистить"
    $s = State
    Check "лист очищен" ($s.doc.text -eq "" -and $s.doc.shapes.Count -eq 0 -and $s.pages -eq 0) "text='$($s.doc.text)' shapes=$($s.doc.shapes.Count) pages=$($s.pages)"
    Call @{cmd = "key"; key = "Z"; ctrl = $true } | Out-Null
    Check "Ctrl+Z после очистки возвращает" ((State).doc.text -eq "что-то") "text='$((State).doc.text)'"
}

Test "Коррекция масштаба" {
    Call @{cmd = "set_config"; config = @{paper = @{origin_x = 54; origin_y = 23 } } } | Out-Null
    Call @{cmd = "set_doc"; doc = @{mode = "Draw"; show_text = $false; show_table = $false; show_image = $false; show_drawing = $true;
            shapes = @(@{Grid = @{a = @(8, 8); b = @(108, 108); step = 10; labels = $false } }) } } | Out-Null
    $a = Call @{cmd = "export" }
    Call @{cmd = "set_config"; config = @{pen = @{scale_x = 1.1; scale_y = 0.9 } } } | Out-Null
    $b = Call @{cmd = "export" }
    $s = State
    Check "масштаб X 110% — G-code в 1.1 раза больше" ([math]::Abs($b.nozzle_x[1] / $a.nozzle_x[1] - 1.1) -lt 0.001) "x: $($a.nozzle_x[1]) -> $($b.nozzle_x[1])"
    Check "превью остаётся 100×100" ([math]::Abs(($s.page_stats[0].preview_bbox[2] - $s.page_stats[0].preview_bbox[0]) - 100) -lt 0.01) "bbox=$($s.page_stats[0].preview_bbox -join ',')"
    Call @{cmd = "set_config"; config = @{pen = @{scale_x = 1; scale_y = 1 }; paper = @{origin_x = $null; origin_y = $null } } } | Out-Null
}

Test "Скрыть настройки" {
    Click "Скрыть настройки"
    Shot "09_no_settings"
    $w = (Call @{cmd = "widgets" }).widgets | Where-Object { $_.label -eq "Формат бумаги" }
    Check "панель настроек скрыта" ($null -eq $w)
    Click "Настройки"
}

Write-Host "`nИтого: $($script:passes) OK, $($script:fails) FAIL. Скриншоты: $Shots" -ForegroundColor $(if ($script:fails) { "Red" } else { "Green" })
exit $script:fails
