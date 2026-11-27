# Remove o HDR Snip de TODOS os usuarios deste PC
$ErrorActionPreference = "Stop"

$isAdmin = ([Security.Principal.WindowsPrincipal] [Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole(
    [Security.Principal.WindowsBuiltInRole]::Administrator
)
if (-not $isAdmin) {
    Start-Process powershell.exe -Verb RunAs -ArgumentList @(
        "-NoProfile", "-ExecutionPolicy", "Bypass", "-File", "`"$PSCommandPath`""
    )
    exit 0
}

Get-Process -Name "hdr-snip" -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep -Milliseconds 300

$lnkPath = "C:\ProgramData\Microsoft\Windows\Start Menu\Programs\Startup\HDR Snip.lnk"
$installDir = "C:\ProgramData\hdr-snip"

if (Test-Path $lnkPath) { Remove-Item $lnkPath -Force }
if (Test-Path $installDir) { Remove-Item $installDir -Recurse -Force }

Write-Host "HDR Snip desinstalado com sucesso de todos os usuarios." -ForegroundColor Yellow
Read-Host "Pressione Enter para fechar"
