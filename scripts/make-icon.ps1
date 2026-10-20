$ErrorActionPreference='Stop'
Add-Type -AssemblyName System.Drawing
$asset=Join-Path (Split-Path $PSScriptRoot) 'assets'
$source=[Drawing.Image]::FromFile((Join-Path (Split-Path $PSScriptRoot) 'controlly-logo.png'))
function Write-Icon([string]$name,[string]$badge){
 $images=@()
 $sizes=if($badge){@(16,20,24,32,48,64)}else{@(16,20,24,32,48,64,128,256)}
 foreach($size in $sizes){
  $bitmap=[Drawing.Bitmap]::new($size,$size);$g=[Drawing.Graphics]::FromImage($bitmap)
  $g.InterpolationMode='HighQualityBicubic';$g.SmoothingMode='AntiAlias';$g.PixelOffsetMode='HighQuality';$g.CompositingQuality='HighQuality'
  $g.DrawImage($source,[Drawing.Rectangle]::new(0,0,$size,$size))
  if($badge){$edge=[Drawing.SolidBrush]::new([Drawing.Color]::White);$fill=[Drawing.SolidBrush]::new([Drawing.ColorTranslator]::FromHtml($badge));$g.FillEllipse($edge,[single]($size*.67),[single]($size*.67),[single]($size*.32),[single]($size*.32));$g.FillEllipse($fill,[single]($size*.7),[single]($size*.7),[single]($size*.26),[single]($size*.26));$edge.Dispose();$fill.Dispose()}
  $stream=[IO.MemoryStream]::new();$bitmap.Save($stream,[Drawing.Imaging.ImageFormat]::Png);$images+=,@($size,$stream.ToArray());if($size -eq 256 -and !$badge){$bitmap.Save("$asset/Connecty.png",[Drawing.Imaging.ImageFormat]::Png)}
  $stream.Dispose();$g.Dispose();$bitmap.Dispose()
 }
 $file=[IO.File]::Create("$asset/$name.ico");$writer=[IO.BinaryWriter]::new($file);$writer.Write([uint16]0);$writer.Write([uint16]1);$writer.Write([uint16]$images.Count);$offset=6+16*$images.Count
 foreach($image in $images){$dimension=if($image[0] -eq 256){0}else{$image[0]};$writer.Write([byte]$dimension);$writer.Write([byte]$dimension);$writer.Write([byte]0);$writer.Write([byte]0);$writer.Write([uint16]1);$writer.Write([uint16]32);$writer.Write([uint32]$image[1].Length);$writer.Write([uint32]$offset);$offset+=$image[1].Length}
 foreach($image in $images){$writer.Write([byte[]]$image[1])};$writer.Dispose()
}
try{Write-Icon 'Connecty' '';Write-Icon 'Connecty-live' '#46AF83';Write-Icon 'Connecty-retry' '#DAA642';Write-Icon 'Connecty-error' '#E26972'}finally{$source.Dispose()}
