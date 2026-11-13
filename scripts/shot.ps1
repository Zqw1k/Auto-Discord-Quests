param([string]$Out = "shot.png", [string]$View = "", [int]$Demo = 1)
$ErrorActionPreference = "Stop"
Add-Type -AssemblyName System.Drawing
Add-Type @"
using System;
using System.Runtime.InteropServices;
public class ShotWin {
  [DllImport("user32.dll")] public static extern bool GetClientRect(IntPtr hWnd, out RECT rect);
  [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr hWnd, IntPtr hdc, uint flags);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hWnd);
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr hWnd, int cmd);
  public struct RECT { public int Left, Top, Right, Bottom; }
}
"@
if ($Demo -eq 1) { $env:QUESTPILOT_DEMO = "1" } else { Remove-Item Env:QUESTPILOT_DEMO -ErrorAction SilentlyContinue }
if ($View) { $env:QUESTPILOT_DEMO_VIEW = $View } else { Remove-Item Env:QUESTPILOT_DEMO_VIEW -ErrorAction SilentlyContinue }

Get-Process questpilot -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep -Milliseconds 800
Start-Process -FilePath ".\target\debug\questpilot.exe" | Out-Null
Start-Sleep -Seconds 3

$proc = Get-Process questpilot -ErrorAction SilentlyContinue |
  Where-Object { $_.MainWindowHandle -ne 0 } |
  Select-Object -First 1
if (-not $proc) { throw "QuestPilot window not found" }

$h = [IntPtr]$proc.MainWindowHandle
[ShotWin]::ShowWindow($h, 9) | Out-Null
[ShotWin]::SetForegroundWindow($h) | Out-Null
Start-Sleep -Milliseconds 800

$client = New-Object ShotWin+RECT
[ShotWin]::GetClientRect($h, [ref]$client) | Out-Null
$w = $client.Right - $client.Left
$ht = $client.Bottom - $client.Top
if ($w -lt 100) { throw "window too small: ${w}x${ht}" }

$bmp = New-Object System.Drawing.Bitmap $w, $ht
$g = [System.Drawing.Graphics]::FromImage($bmp)
$hdc = $g.GetHdc()
[ShotWin]::PrintWindow($h, $hdc, 2) | Out-Null
$g.ReleaseHdc($hdc)
$bmp.Save((Join-Path $PWD $Out), [System.Drawing.Imaging.ImageFormat]::Png)
$g.Dispose()
$bmp.Dispose()
Write-Output "saved $Out ($w x $ht)"
