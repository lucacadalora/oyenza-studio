# A stand-in music player for the weight check (CI): it tells Windows a song
# is playing, the way Apple Music or Spotify do, without playing a sound (CI
# machines have no audio device). Oyen Poni then shows its music pill with
# the equalizer, and tools/ci-weight.ps1 measures what that costs.
#
# Windows PowerShell 5.1 only (WinRT types don't load in pwsh 7):
#   powershell.exe -NoProfile -ExecutionPolicy Bypass -File tools\ci-fake-music.ps1 -Seconds 60 -Ready <file>
# -Ready names a file it writes once the session says "playing", or the error.
param(
    [int]$Seconds = 60,
    [string]$Ready = ""
)

$ErrorActionPreference = "Stop"
try {
    $null = [Windows.Media.Playback.MediaPlayer, Windows.Media.Playback, ContentType = WindowsRuntime]
    $null = [Windows.Media.SystemMediaTransportControlsTimelineProperties, Windows.Media, ContentType = WindowsRuntime]

    # A media player with its automatic controls off is an empty session the
    # script drives by hand: nothing is loaded and nothing plays.
    $player = New-Object Windows.Media.Playback.MediaPlayer
    $player.CommandManager.IsEnabled = $false
    $smtc = $player.SystemMediaTransportControls
    $smtc.IsEnabled = $true
    $smtc.IsPlayEnabled = $true
    $smtc.IsPauseEnabled = $true
    $smtc.IsNextEnabled = $true
    $smtc.IsPreviousEnabled = $true

    $d = $smtc.DisplayUpdater
    $d.Type = [Windows.Media.MediaPlaybackType]::Music
    $d.MusicProperties.Title = "Weight check"
    $d.MusicProperties.Artist = "Oyen Poni CI"
    $d.Update()

    $t = New-Object Windows.Media.SystemMediaTransportControlsTimelineProperties
    $t.StartTime = [TimeSpan]::Zero
    $t.MinSeekTime = [TimeSpan]::Zero
    $t.EndTime = [TimeSpan]::FromSeconds(240)
    $t.MaxSeekTime = [TimeSpan]::FromSeconds(240)
    $t.Position = [TimeSpan]::FromSeconds(30)
    $smtc.UpdateTimelineProperties($t)
    $smtc.PlaybackStatus = [Windows.Media.MediaPlaybackStatus]::Playing

    if ($Ready) { "playing" | Set-Content -Path $Ready }
    Start-Sleep -Seconds $Seconds
    $smtc.PlaybackStatus = [Windows.Media.MediaPlaybackStatus]::Stopped
} catch {
    if ($Ready) { "failed: $($_.Exception.Message)" | Set-Content -Path $Ready }
    exit 1
}
