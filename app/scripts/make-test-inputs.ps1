# Writes a hand made input track (<name>.inputs.json) next to the newest Valorant test
# clip, in the format of Klip-Vault\Recorder\Eingabe-Erfassung.md. Nothing here was ever
# typed by anyone: the presses follow a fixed pattern so that the overlay can be checked
# against known times.

$ErrorActionPreference = "Stop"
$clips = Join-Path (Split-Path $PSScriptRoot -Parent) "testdata\clips\Valorant"
$video = Get-ChildItem $clips -Filter *.mp4 | Sort-Object Name -Descending | Select-Object -First 1
if (-not $video) { throw "no test clip in $clips, run make-testdata.ps1 first" }
$duration = 30000

$events = New-Object System.Collections.Generic.List[object]
function Press([int]$at, [string]$kind, [int]$code, [int]$holdMs) {
    $events.Add(@($at, $kind, $code, 1))
    $events.Add(@(($at + $holdMs), $kind, $code, 0))
}

# Scancodes: W 17, A 30, S 31, D 32, Space 57, left Shift 42, left Ctrl 29, 1 2, 2 3, R 19, E 18.
# W is already down when the clip starts and is let go at 4 s.
$events.Add(@(4000, "k", 17, 0))
Press 1000 "k" 30 700      # A from 1.0 to 1.7 s
Press 2000 "k" 32 900      # D from 2.0 to 2.9 s
Press 3000 "k" 57 150      # Space at 3.0 s
Press 5000 "k" 17 6000     # W from 5 to 11 s
Press 5500 "k" 42 2500     # Shift from 5.5 to 8 s
foreach ($kill in 6200, 9100, 12800, 17400, 21000) {
    Press $kill "b" 0 120                 # left click on every kill of the sidecar
    Press ($kill - 600) "k" 30 350        # strafe before the shot
    Press ($kill - 200) "k" 32 300
}
Press 8200 "b" 1 900       # right button from 8.2 to 9.1 s
Press 14000 "k" 29 1800    # Ctrl from 14 to 15.8 s
Press 15000 "k" 2 100      # 1
Press 15400 "k" 3 100      # 2
Press 19000 "k" 19 200     # R
Press 23000 "k" 18 200     # E
Press 24000 "k" 31 2000    # S from 24 to 26 s
Press 27000 "b" 2 200      # middle button
$events.Add(@(10000, "w", 120, 0))
$events.Add(@(10300, "w", -120, 0))
$events.Add(@(16000, "m", 0, 240))

$sorted = $events | Sort-Object { $_[0] }
$parts = $sorted | ForEach-Object { '[{0},"{1}",{2},{3}]' -f $_[0], $_[1], $_[2], $_[3] }
$json = '{"version":1,"duration_ms":' + $duration + ',"move_window_ms":50,"held":{"keys":[17],"buttons":[]},"events":[' + ($parts -join ",") + ']}'
$out = [IO.Path]::ChangeExtension($video.FullName, "inputs.json")
[IO.File]::WriteAllText($out, $json, (New-Object System.Text.UTF8Encoding($false)))
Write-Host "ok  $out ($($sorted.Count) Ereignisse)"
