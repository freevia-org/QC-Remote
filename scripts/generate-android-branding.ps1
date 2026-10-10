# Generates the Android app's launcher-icon and splash-screen bitmaps from
# the shared QC Remote brand icon and the @qc-remote/theme color/typography
# contract. It redraws every existing ic_launcher*.png and splash.png found
# under the Android resource tree in place, at each file's own existing
# dimensions, so the Capacitor/Android-Studio-generated density buckets never
# need to be hand-maintained.
#
# Scoped to the Android target only: this repo's Windows app has its own
# icon pipeline and is out of scope here. The default source icon is the
# rasterized brand icon already checked into apps/windows/src-tauri/icons
# (produced from packages/typescript/qc-theme/assets/app-icon.svg); it is
# only ever read, never modified.

param(
    [string]$SourceIcon
)

$ErrorActionPreference = "Stop"

$repoRoot = Split-Path -Parent $PSScriptRoot
$androidRoot = Join-Path $repoRoot "apps\android\android\app\src\main\res"

if (-not $SourceIcon) {
    $SourceIcon = Join-Path $repoRoot "apps\windows\src-tauri\icons\icon.png"
}
$sourceIcon = (Resolve-Path -LiteralPath $SourceIcon -ErrorAction Stop).Path

$nativeTheme = Get-Content -LiteralPath (Join-Path $repoRoot "packages\typescript\qc-theme\src\native-theme.json") -Raw | ConvertFrom-Json
$brand = Get-Content -LiteralPath (Join-Path $repoRoot "packages\typescript\qc-theme\src\brand.json") -Raw | ConvertFrom-Json

Add-Type -AssemblyName System.Drawing
$fontCollection = [System.Drawing.Text.PrivateFontCollection]::new()
$regularFontPath = Join-Path $repoRoot ("packages\typescript\qc-theme\" + $nativeTheme.android.splashFontRegular.Replace('/', '\'))
$boldFontPath = Join-Path $repoRoot ("packages\typescript\qc-theme\" + $nativeTheme.android.splashFontBold.Replace('/', '\'))
$fontCollection.AddFontFile((Resolve-Path -LiteralPath $regularFontPath -ErrorAction Stop).Path)
$fontCollection.AddFontFile((Resolve-Path -LiteralPath $boldFontPath -ErrorAction Stop).Path)
$splashFontFamily = $fontCollection.Families | Where-Object Name -eq $nativeTheme.android.splashFontFamily | Select-Object -First 1
if ($null -eq $splashFontFamily) {
    throw "Bundled splash font family '$($nativeTheme.android.splashFontFamily)' was not loaded."
}

function New-Canvas([int]$width, [int]$height, [bool]$transparent = $false) {
    $bitmap = [System.Drawing.Bitmap]::new($width, $height, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
    $graphics.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::HighQuality
    $graphics.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
    $graphics.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality
    $graphics.TextRenderingHint = [System.Drawing.Text.TextRenderingHint]::AntiAliasGridFit
    if ($transparent) { $graphics.Clear([System.Drawing.Color]::Transparent) } else { $graphics.Clear([System.Drawing.ColorTranslator]::FromHtml($nativeTheme.android.background)) }
    return @($bitmap, $graphics)
}

function Save-Png($bitmap, $graphics, [string]$path) {
    $graphics.Dispose()
    $bitmap.Save($path, [System.Drawing.Imaging.ImageFormat]::Png)
    $bitmap.Dispose()
}

$icon = [System.Drawing.Image]::FromFile($sourceIcon)
try {
    Get-ChildItem $androidRoot -Recurse -File -Filter "ic_launcher*.png" | ForEach-Object {
        $existing = [System.Drawing.Image]::FromFile($_.FullName)
        $width = $existing.Width
        $height = $existing.Height
        $existing.Dispose()
        $isForeground = $_.BaseName -eq "ic_launcher_foreground"
        $canvas = New-Canvas $width $height $isForeground
        $bitmap, $graphics = $canvas
        $scale = if ($isForeground) { 0.69 } else { 1.0 }
        $size = [int]([Math]::Min($width, $height) * $scale)
        $left = [int](($width - $size) / 2)
        $top = [int](($height - $size) / 2)
        $graphics.DrawImage($icon, $left, $top, $size, $size)
        Save-Png $bitmap $graphics $_.FullName
    }

    Get-ChildItem $androidRoot -Recurse -File -Filter "splash.png" | ForEach-Object {
        $existing = [System.Drawing.Image]::FromFile($_.FullName)
        $width = $existing.Width
        $height = $existing.Height
        $existing.Dispose()
        $canvas = New-Canvas $width $height
        $bitmap, $graphics = $canvas
        $shortEdge = [Math]::Min($width, $height)
        $logoSize = [int]($shortEdge * 0.34)
        $contentHeight = [int]($logoSize + $shortEdge * 0.16)
        $logoTop = [int](($height - $contentHeight) / 2)
        $graphics.DrawImage($icon, [int](($width - $logoSize) / 2), $logoTop, $logoSize, $logoSize)

        $brandSize = [Math]::Max(10, [int]($shortEdge * 0.055))
        $captionSize = [Math]::Max(7, [int]($shortEdge * 0.024))
        $brandFont = [System.Drawing.Font]::new($splashFontFamily, $brandSize, [System.Drawing.FontStyle]::Bold, [System.Drawing.GraphicsUnit]::Pixel)
        $captionFont = [System.Drawing.Font]::new($splashFontFamily, $captionSize, [System.Drawing.FontStyle]::Regular, [System.Drawing.GraphicsUnit]::Pixel)
        $center = [System.Drawing.StringFormat]::new()
        $center.Alignment = [System.Drawing.StringAlignment]::Center
        $brandBrush = [System.Drawing.SolidBrush]::new([System.Drawing.ColorTranslator]::FromHtml($nativeTheme.android.splashText))
        $captionBrush = [System.Drawing.SolidBrush]::new([System.Drawing.ColorTranslator]::FromHtml($nativeTheme.android.splashCaption))
        $textTop = $logoTop + $logoSize + [int]($shortEdge * 0.045)
        $graphics.DrawString($brand.appWordmark, $brandFont, $brandBrush, [System.Drawing.RectangleF]::new(0, $textTop, $width, $brandSize * 1.35), $center)
        $graphics.DrawString($brand.companionCaption, $captionFont, $captionBrush, [System.Drawing.RectangleF]::new(0, $textTop + $brandSize * 1.5, $width, $captionSize * 1.4), $center)
        $brandFont.Dispose(); $captionFont.Dispose(); $center.Dispose(); $brandBrush.Dispose(); $captionBrush.Dispose()
        Save-Png $bitmap $graphics $_.FullName
    }
}
finally {
    $icon.Dispose()
    $fontCollection.Dispose()
}

Write-Output "Generated Android launcher and splash assets from the $($brand.appName) master icon ($sourceIcon)."
