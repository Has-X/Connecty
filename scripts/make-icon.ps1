$ErrorActionPreference='Stop'
Add-Type -AssemblyName System.Drawing
$asset=Join-Path (Split-Path $PSScriptRoot) 'assets'
$source=[Drawing.Image]::FromFile((Join-Path (Split-Path $PSScriptRoot) 'controlly-logo.png'))
function Get-PixelHash([Drawing.Bitmap]$bitmap){
 $rectangle=[Drawing.Rectangle]::new(0,0,$bitmap.Width,$bitmap.Height)
 $data=$bitmap.LockBits($rectangle,[Drawing.Imaging.ImageLockMode]::ReadOnly,[Drawing.Imaging.PixelFormat]::Format32bppArgb)
 try{
  $rowBytes=$bitmap.Width*4
  $pixels=[byte[]]::new($rowBytes*$bitmap.Height)
  for($row=0;$row -lt $bitmap.Height;$row++){
   [Runtime.InteropServices.Marshal]::Copy([IntPtr]::Add($data.Scan0,$row*$data.Stride),$pixels,$row*$rowBytes,$rowBytes)
  }
  $hash=[Security.Cryptography.SHA256]::Create()
  try{return [Convert]::ToBase64String($hash.ComputeHash($pixels))}finally{$hash.Dispose()}
 }finally{$bitmap.UnlockBits($data)}
}
function Save-Logo([Drawing.Bitmap]$bitmap,[string]$path){
 if(Test-Path -LiteralPath $path){
  $existing=[Drawing.Bitmap]::new($path)
  try{
   # Retain lossless PNG compression when only the source encoding changed.
   if($existing.Size -eq $bitmap.Size -and (Get-PixelHash $existing) -eq (Get-PixelHash $bitmap)){return}
  }finally{$existing.Dispose()}
 }
 $bitmap.Save($path,[Drawing.Imaging.ImageFormat]::Png)
}
function Write-Icon([string]$name,[string]$badge){
 $images=@()
 $isTray=$name -ne 'Connecty'
 $sizes=if($isTray){@(16,20,24,32,48,64)}else{@(16,20,24,32,48,64,128,256)}
 foreach($size in $sizes){
  $bitmap=[Drawing.Bitmap]::new($size,$size);$g=[Drawing.Graphics]::FromImage($bitmap)
  $g.InterpolationMode='HighQualityBicubic';$g.SmoothingMode='AntiAlias';$g.PixelOffsetMode='HighQuality';$g.CompositingQuality='HighQuality'
  $g.DrawImage($source,[Drawing.Rectangle]::new(0,0,$size,$size))
  if($badge){
   # Preserve the original brand; enlarge only the status badge, without a rim.
   $fill=[Drawing.SolidBrush]::new([Drawing.ColorTranslator]::FromHtml($badge))
   $g.FillEllipse($fill,[single]($size*.58),[single]($size*.58),[single]($size*.42),[single]($size*.42))
   $fill.Dispose()
  }
  $stream=[IO.MemoryStream]::new();$bitmap.Save($stream,[Drawing.Imaging.ImageFormat]::Png);$images+=,@($size,$stream.ToArray());if($size -eq 256 -and !$isTray){Save-Logo $bitmap "$asset/Connecty.png"}
  $stream.Dispose();$g.Dispose();$bitmap.Dispose()
 }
 $file=[IO.File]::Create("$asset/$name.ico");$writer=[IO.BinaryWriter]::new($file);$writer.Write([uint16]0);$writer.Write([uint16]1);$writer.Write([uint16]$images.Count);$offset=6+16*$images.Count
 foreach($image in $images){$dimension=if($image[0] -eq 256){0}else{$image[0]};$writer.Write([byte]$dimension);$writer.Write([byte]$dimension);$writer.Write([byte]0);$writer.Write([byte]0);$writer.Write([uint16]1);$writer.Write([uint16]32);$writer.Write([uint32]$image[1].Length);$writer.Write([uint32]$offset);$offset+=$image[1].Length}
 foreach($image in $images){$writer.Write([byte[]]$image[1])};$writer.Dispose()
}
try{Write-Icon 'Connecty' '';Write-Icon 'Connecty-tray' '';Write-Icon 'Connecty-live' '#46AF83';Write-Icon 'Connecty-retry' '#DAA642';Write-Icon 'Connecty-error' '#E26972'}finally{$source.Dispose()}
