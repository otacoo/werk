param(
    [string]$Out = (Join-Path $PSScriptRoot "..\src-tauri\app-icon.png"),
    [int]$Size = 1024
)
$ErrorActionPreference = "Stop"
Add-Type -AssemblyName System.Drawing

$bmp = New-Object System.Drawing.Bitmap($Size, $Size, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
$g.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality
$g.Clear([System.Drawing.Color]::Transparent)

$s = $Size / 512.0

function New-RoundedPath([double]$x, [double]$y, [double]$w, [double]$h, [double]$r) {
    $p = New-Object System.Drawing.Drawing2D.GraphicsPath
    $d = $r * 2
    $p.AddArc($x, $y, $d, $d, 180, 90)
    $p.AddArc($x + $w - $d, $y, $d, $d, 270, 90)
    $p.AddArc($x + $w - $d, $y + $h - $d, $d, $d, 0, 90)
    $p.AddArc($x, $y + $h - $d, $d, $d, 90, 90)
    $p.CloseFigure()
    return $p
}

function New-Stroke([double[][]]$points) {
    $path = New-Object System.Drawing.Drawing2D.GraphicsPath
    $pts = @()
    foreach ($pt in $points) {
        $pts += New-Object System.Drawing.PointF([single]($pt[0] * $s), [single]($pt[1] * $s))
    }
    $path.AddLines([System.Drawing.PointF[]]$pts)
    return $path
}

# Graphite tile: keeps the mark legible on light and dark surfaces.
$radius = 115 * $s
$fill = New-Object System.Drawing.Drawing2D.LinearGradientBrush(
    (New-Object System.Drawing.RectangleF(0, 0, $Size, $Size)),
    [System.Drawing.Color]::FromArgb(255, 44, 49, 56),
    [System.Drawing.Color]::FromArgb(255, 20, 22, 25),
    90.0)
$g.FillPath($fill, (New-RoundedPath 0 0 $Size $Size $radius))

# Wordmark strokes: same geometry as public/logo.svg.
$pen = New-Object System.Drawing.Pen([System.Drawing.Color]::White, [single](46 * $s))
$pen.StartCap = [System.Drawing.Drawing2D.LineCap]::Round
$pen.EndCap = [System.Drawing.Drawing2D.LineCap]::Round
$pen.LineJoin = [System.Drawing.Drawing2D.LineJoin]::Round
$g.DrawPath($pen, (New-Stroke @(@(55, 200), @(105, 326), @(153, 248), @(201, 326), @(251, 200))))
$g.DrawPath($pen, (New-Stroke @(@(305, 180), @(305, 326))))
$g.DrawPath($pen, (New-Stroke @(@(363, 212), @(321, 254), @(379, 326))))

$dot = New-Object System.Drawing.SolidBrush([System.Drawing.Color]::FromArgb(255, 59, 130, 246))
$g.FillEllipse($dot, [single](416 * $s), [single](285 * $s), [single](64 * $s), [single](64 * $s))

$g.Dispose()
$dir = Split-Path -Parent $Out
if (!(Test-Path $dir)) { New-Item -ItemType Directory -Path $dir | Out-Null }
$bmp.Save($Out, [System.Drawing.Imaging.ImageFormat]::Png)
$bmp.Dispose()
Write-Output "wrote $Out"
