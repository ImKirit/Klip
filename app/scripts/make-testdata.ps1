# Builds the test library under app\testdata\clips with ffmpeg test sources.
# Every clip gets a .klip.json sidecar like the recorder writes it. Clips with sound
# carry several audio tracks: track 1 is the mix, the others are the single sources.
# Never point this at a real clip folder, it wipes its target first.

param([string]$Ffmpeg = "C:\ffmpeg\bin\ffmpeg.exe")

$ErrorActionPreference = "Stop"
$root = Join-Path (Split-Path $PSScriptRoot -Parent) "testdata"
$clips = Join-Path $root "clips"
if ($clips -notlike "*\app\testdata\clips") { throw "refusing to wipe $clips" }
if (Test-Path $clips) { [IO.Directory]::Delete($clips, $true) }
foreach ($d in "clips", "data", "cache", "out") { New-Item -ItemType Directory -Force (Join-Path $root $d) | Out-Null }

$utf8 = New-Object System.Text.UTF8Encoding($false)

function Write-Sidecar($video, $meta) {
    $path = [IO.Path]::ChangeExtension($video, "klip.json")
    [IO.File]::WriteAllText($path, ($meta | ConvertTo-Json -Depth 6), $utf8)
}

function Stamp([double]$daysAgo, [int]$hour, [int]$minute) {
    $d = (Get-Date).Date.AddDays(-$daysAgo).AddHours($hour).AddMinutes($minute)
    if ($d -gt (Get-Date)) { $d = (Get-Date).AddMinutes(-5) }
    return $d
}

# Sound sources that look different as waveforms.
$voices = @{
    "Spiel"    = "sine=f=196:d={0},tremolo=f=3:d=0.7"
    "Mikrofon" = "sine=f=587:d={0},volume='if(lt(mod(t,3),1.2),0.9,0.02)':eval=frame"
    "Discord"  = "anoisesrc=c=pink:d={0}:a=0.4,volume='if(gt(mod(t,5),3),1,0.03)':eval=frame"
    "Musik"    = "sine=f=330:d={0},tremolo=f=0.5:d=0.9,volume=0.5"
}

function New-Clip {
    param(
        [string]$Game, [string]$Folder, [string]$Kind, [string]$Source, [int]$Seconds,
        [double]$DaysAgo, [int]$Hour, [int]$Minute, [string[]]$Tracks = @(),
        [object[]]$Events = @(), [bool]$Favorite = $false, [string[]]$Tags = @(),
        [string]$Codec = "h264", [int]$Width = 1280, [int]$Height = 720, [int]$Fps = 60,
        [string]$Suffix = "", [bool]$Sidecar = $true
    )
    $when = Stamp $DaysAgo $Hour $Minute
    $dir = Join-Path $clips $Folder
    New-Item -ItemType Directory -Force $dir | Out-Null
    $name = "Klip_" + $when.ToString("yyyy-MM-dd_HH-mm-ss") + $Suffix
    $video = Join-Path $dir "$name.mp4"

    $inputs = @("-f", "lavfi", "-i", "$Source,scale=${Width}:${Height},setsar=1,fps=$Fps,format=yuv420p")
    $maps = @("-map", "0:v")
    $filter = ""
    $names = @()
    if ($Tracks.Count -gt 0) {
        $i = 1
        $mixIn = ""
        foreach ($t in $Tracks) {
            $inputs += @("-f", "lavfi", "-i", ($voices[$t] -f $Seconds))
            $filter += "[${i}:a]aformat=sample_rates=48000:channel_layouts=stereo,asplit[s$i][m$i];"
            $mixIn += "[m$i]"
            $i++
        }
        $filter += "${mixIn}amix=inputs=$($Tracks.Count):normalize=0[mix]"
        $maps += @("-map", "[mix]")
        for ($k = 1; $k -lt $i; $k++) { $maps += @("-map", "[s$k]") }
        $names = @("Mix") + $Tracks
    }
    $vcodec = switch ($Codec) {
        "hevc" { @("-c:v", "libx265", "-preset", "veryfast", "-x265-params", "log-level=error", "-tag:v", "hvc1") }
        "av1" { @("-c:v", "libsvtav1", "-preset", "10", "-svtav1-params", "keyint=2s") }
        default { @("-c:v", "libx264", "-preset", "veryfast", "-g", "$Fps") }
    }
    $cmd = @("-hide_banner", "-loglevel", "error", "-y") + $inputs
    if ($filter) { $cmd += @("-filter_complex", $filter) }
    $cmd += $maps + $vcodec + @("-t", "$Seconds", "-b:v", "3M")
    if ($Tracks.Count -gt 0) { $cmd += @("-c:a", "aac", "-b:a", "128k") }
    $cmd += @("-movflags", "+faststart", $video)
    & $Ffmpeg @cmd
    if ($LASTEXITCODE -ne 0) { throw "ffmpeg failed for $video" }

    if ($Sidecar) {
        $meta = [ordered]@{
            version = 1; kind = $Kind; created = $when.ToString("yyyy-MM-ddTHH:mm:sszzz")
            duration_ms = $Seconds * 1000; width = $Width; height = $Height; fps = $Fps
            codec = $Codec; encoder = "Testdaten (ffmpeg)"; audio_tracks = $names
            events = $Events; favorite = $Favorite; tags = $Tags
        }
        if ($Game) { $meta.game = $Game }
        Write-Sidecar $video $meta
    }
    Write-Host "ok  $video"
}

function Ev([int]$ms, [string]$kind, [string]$label) {
    $e = [ordered]@{ at_ms = $ms; kind = $kind }
    if ($label) { $e.label = $label }
    return $e
}

$val = "Valorant"; $trio = @("Spiel", "Mikrofon", "Discord")

New-Clip -Game $val -Folder $val -Kind "auto_clip" -Source "testsrc2=d=30" -Seconds 30 -DaysAgo 0 -Hour 14 -Minute 22 `
    -Tracks $trio -Favorite $true -Tags @("ranked", "ascent") -Events @(
        (Ev 6200 "kill"), (Ev 9100 "kill"), (Ev 12800 "kill"), (Ev 17400 "kill"), (Ev 21000 "kill"), (Ev 21000 "ace"), (Ev 26500 "round_win"))
New-Clip -Game $val -Folder $val -Kind "auto_clip" -Source "testsrc2=d=14,hue=h=90" -Seconds 14 -DaysAgo 0 -Hour 13 -Minute 51 `
    -Tracks $trio -Tags @("ranked") -Events @((Ev 5000 "kill"), (Ev 7600 "kill"), (Ev 7600 "double_kill"))
New-Clip -Game $val -Folder $val -Kind "clip" -Source "smptehdbars=d=20" -Seconds 20 -DaysAgo 0 -Hour 11 -Minute 5 -Tracks @("Spiel", "Mikrofon")
New-Clip -Game $val -Folder $val -Kind "recording" -Source "testsrc2=d=95,hue=h=200" -Seconds 95 -DaysAgo 1 -Hour 21 -Minute 40 `
    -Tracks @("Spiel", "Mikrofon", "Discord", "Musik") -Tags @("scrim") -Events @(
        (Ev 12000 "marker" "Eco-Runde"), (Ev 41000 "marker"), (Ev 43500 "kill"), (Ev 70500 "marker" "Clutch 1v3"), (Ev 73000 "clutch"))
New-Clip -Game $val -Folder $val -Kind "auto_clip" -Source "testsrc2=d=12,hue=h=300" -Seconds 12 -DaysAgo 1 -Hour 20 -Minute 12 `
    -Tracks $trio -Events @((Ev 4800 "death")) -Tags @("fail")
New-Clip -Game "Counter-Strike 2" -Folder "Counter-Strike 2" -Kind "clip" -Source "testsrc2=d=12,negate" -Seconds 12 -DaysAgo 3 -Hour 19 -Minute 30 `
    -Tracks @("Spiel", "Mikrofon") -Favorite $true -Tags @("faceit")
New-Clip -Game "Counter-Strike 2" -Folder "Counter-Strike 2" -Kind "auto_clip" -Source "pal75bars=d=10" -Seconds 10 -DaysAgo 9 -Hour 22 -Minute 3 `
    -Tracks @("Spiel", "Mikrofon") -Events @((Ev 3900 "kill"), (Ev 5200 "kill"), (Ev 6000 "kill"), (Ev 6000 "triple_kill"))
New-Clip -Game "Minecraft" -Folder "Minecraft" -Kind "recording" -Source "cellauto=rule=110:rate=60:s=320x180" -Seconds 45 -DaysAgo 44 -Hour 16 -Minute 18 `
    -Tracks @("Spiel", "Musik") -Events @((Ev 20000 "marker" "Basis fertig")) -Tags @("survival")
New-Clip -Game "Minecraft" -Folder "Minecraft" -Kind "clip" -Source "rgbtestsrc=d=10" -Seconds 10 -DaysAgo 44 -Hour 17 -Minute 2 `
    -Tracks @("Spiel", "Mikrofon") -Codec "hevc" -Suffix "_hevc"
New-Clip -Game "Minecraft" -Folder "Minecraft" -Kind "clip" -Source "yuvtestsrc=d=10" -Seconds 10 -DaysAgo 60 -Hour 17 -Minute 30 `
    -Tracks @("Spiel", "Mikrofon") -Codec "av1" -Suffix "_av1"
# Like the recorder writes today: no sound, no game.
New-Clip -Game "" -Folder "" -Kind "clip" -Source "testsrc=d=8" -Seconds 8 -DaysAgo 3 -Hour 10 -Minute 0 -Width 1920 -Height 1080
# A video somebody copied into the folder by hand, without a sidecar.
New-Clip -Game "" -Folder "" -Kind "clip" -Source "smptebars=d=6" -Seconds 6 -DaysAgo 5 -Hour 9 -Minute 15 -Tracks @("Spiel") -Sidecar $false -Suffix "_ohne_begleitdatei"

# Screenshot with sidecar.
$when = Stamp 1 20 30
$shot = Join-Path $clips ("Valorant\Klip_" + $when.ToString("yyyy-MM-dd_HH-mm-ss") + ".png")
& $Ffmpeg -hide_banner -loglevel error -y -f lavfi -i "testsrc2=s=1920x1080,hue=h=140" -frames:v 1 $shot
Write-Sidecar $shot ([ordered]@{
        version = 1; kind = "screenshot"; created = $when.ToString("yyyy-MM-ddTHH:mm:sszzz"); game = "Valorant"
        duration_ms = 0; width = 1920; height = 1080; fps = 0; codec = "h264"; encoder = "png"
        audio_tracks = @(); events = @(); favorite = $false; tags = @()
    })
Write-Host "ok  $shot"
Write-Host "Testdaten liegen in $clips"

& (Join-Path $PSScriptRoot "make-test-inputs.ps1")
