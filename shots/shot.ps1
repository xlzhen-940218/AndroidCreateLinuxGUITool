param(
  [Parameter(Mandatory = $true)][string]$Out,
  [int]$DelayMs = 0
)
Add-Type -AssemblyName System.Windows.Forms, System.Drawing
if ($DelayMs -gt 0) { Start-Sleep -Milliseconds $DelayMs }
$b = [System.Windows.Forms.SystemInformation]::VirtualScreen
$bmp = New-Object System.Drawing.Bitmap($b.Width, $b.Height)
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.CopyFromScreen($b.Left, $b.Top, 0, 0, $bmp.Size)
$g.Dispose()
$dir = Split-Path -Parent $Out
if ($dir -and -not (Test-Path $dir)) { New-Item -ItemType Directory -Path $dir -Force | Out-Null }
$bmp.Save($Out, [System.Drawing.Imaging.ImageFormat]::Png)
$bmp.Dispose()
Write-Output "saved: $Out ($($b.Width)x$($b.Height))"
