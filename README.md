# HDR Snip (`hdr-snip.exe`)

Utilitário nativo e levíssimo (~374 KB, ~4 MB RAM em repouso, 0% CPU) em Rust para tirar prints no **Windows 11 com HDR ligado** com **branco puro (`255`) e cores exatas** — sem o estouro branco do Lightshot e sem o cinza lavado (`224`) do Snipping Tool.

## Como funciona
- Captura via WinRT `Windows.Graphics.Capture` em **`R16G16B16A16Float` (FP16 scRGB)** sem clipping.
- Lê dinamicamente o **`DISPLAYCONFIG_SDR_WHITE_LEVEL`** do monitor sob o cursor (ex.: `240 nits = 3.00x`).
- Divide o buffer linear FP16 pelo multiplicador SDR exato e aplica a curva **sRGB OETF** via LUT 16K.
- Abre overlay instantâneo com UI **Keycap Minimal** (renderização vetorial anti-aliased nativa + escala Per-Monitor DPI), cantos em L de alto contraste, dimensões em tempo real e barra flutuante (**Copiar `↵`**, **Salvar `S`**, **Tela `F`**, **`✕ Esc`**).

## Atalhos
- **`PrintScreen`** ou **`Win + Shift + S`**: congela a tela e abre o recorte.
- Dentro do recorte: **`Enter`** / **`C`** / **`Ctrl+C`** / **duplo clique** (copia), **`S`** / **`Ctrl+S`** (salva PNG), **`F`** (tela toda), **`Esc`** / **botão direito** (cancela).
- Ícone na bandeja do sistema: clique esquerdo captura na hora; clique direito mostra status HDR ao vivo, pausar atalhos e sair.

## Instalar após formatar o PC (sem precisar instalar Rust!)
1. Baixe `hdr-snip.exe` e `install_all_users.ps1` na aba **Releases** deste repositório na mesma pasta.
2. Rode no PowerShell:
```powershell
powershell -ExecutionPolicy Bypass -File .\install_all_users.ps1
```

## Ou compilar do zero e instalar para todos os usuários
```powershell
cargo build --release
.\install_all_users.ps1
```
Instalado em `C:\ProgramData\hdr-snip\hdr-snip.exe` com inicialização automática em `C:\ProgramData\Microsoft\Windows\Start Menu\Programs\Startup\HDR Snip.lnk`.
