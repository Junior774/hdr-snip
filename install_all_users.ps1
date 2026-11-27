# Instala o HDR Snip para TODOS os usuários deste PC (Windows 11)
# Requer PowerShell como Administrador para escrever em C:\ProgramData\...\Startup.

$ErrorActionPreference = "Stop"

$isAdmin = ([Security.Principal.WindowsPrincipal] [Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole(
    [Security.Principal.WindowsBuiltInRole]::Administrator
)
if (-not $isAdmin) {
    Write-Host "Reabrindo como Administrador (UAC) para instalar para TODOS os usuarios..." -ForegroundColor Yellow
    Start-Process powershell.exe -Verb RunAs -ArgumentList @(
        "-NoProfile", "-ExecutionPolicy", "Bypass", "-File", "`"$PSCommandPath`""
    )
    exit 0
}

$exeSrc = Join-Path $PSScriptRoot "target\release\hdr-snip.exe"
if (-not (Test-Path $exeSrc)) {
    Write-Error "Executavel nao encontrado em $exeSrc. Rode 'cargo build --release' primeiro."
}

$installDir = "C:\ProgramData\hdr-snip"
$exeDst = Join-Path $installDir "hdr-snip.exe"
$startupDir = "C:\ProgramData\Microsoft\Windows\Start Menu\Programs\Startup"
$lnkPath = Join-Path $startupDir "HDR Snip.lnk"

# Encerra instancias anteriores antes de copiar o binario novo
Get-Process -Name "hdr-snip" -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep -Milliseconds 300

New-Item -ItemType Directory -Path $installDir -Force | Out-Null
Copy-Item -Path $exeSrc -Destination $exeDst -Force

# Garante permissao de leitura e execucao para TODOS os usuarios do PC
icacls $installDir /grant "*S-1-5-32-545:(OI)(CI)RX" /T /Q | Out-Null

# Cria atalho no Startup de TODOS os usuarios
$wsh = New-Object -ComObject WScript.Shell
$shortcut = $wsh.CreateShortcut($lnkPath)
$shortcut.TargetPath = $exeDst
$shortcut.WorkingDirectory = $installDir
$shortcut.Description = "HDR Snip - Prints HDR->sRGB perfeitos (PrintScreen / Win+Shift+S)"
$shortcut.Save()

Write-Host ""
Write-Host "HDR Snip instalado com sucesso para TODOS os usuarios do Windows!" -ForegroundColor Green
Write-Host "  Binario : $exeDst"
Write-Host "  Startup : $lnkPath"
Write-Host ""
Write-Host "Dica: se voce tinha o Lightshot aberto, feche/desative ele no Iniciar para nao brigar pelo PrintScreen." -ForegroundColor Cyan

# Inicia imediatamente para a sessao atual
Start-Process -FilePath $exeDst -WorkingDirectory $installDir
Write-Host "HDR Snip iniciado na bandeja do sistema! Teste agora com PrintScreen ou Win+Shift+S." -ForegroundColor Green
Write-Host ""
Read-Host "Pressione Enter para fechar"
