# HDR Snip (`hdr-snip.exe`)

Utilitário nativo e levíssimo (~1–2 MB, ~4 MB RAM em repouso, 0% CPU) em Rust para tirar prints no **Windows 11 com HDR ligado** com **branco puro (`255`) e cores exatas** — sem o estouro branco do Lightshot e sem o cinza lavado (`224`) do Snipping Tool.

## Como funciona
- Captura via WinRT `Windows.Graphics.Capture` em **`R16G16B16A16Float` (FP16 scRGB)** sem clipping.
- Lê dinamicamente o **`DISPLAYCONFIG_SDR_WHITE_LEVEL`** do monitor sob o cursor (ex.: `240 nits = 3.00x`).
- Divide o buffer linear FP16 pelo multiplicador SDR exato e aplica a curva **sRGB OETF** via LUT 16K.
- Abre overlay instantâneo estilo Lightshot com alças de redimensionamento, dimensões em tempo real e barra flutuante (**Copiar**, **Salvar PNG**, **Tela toda**, **Cancelar**).

## Atalhos
- **`PrintScreen`** ou **`Win + Shift + S`**: congela a tela e abre o recorte.
- Dentro do recorte: **`Ctrl+C`** / **`Enter`** / **duplo clique** (copia), **`Ctrl+S`** (salva PNG), **`F`** (tela toda), **`Esc`** / **botão direito** (cancela).
- Ícone na bandeja do sistema: clique esquerdo captura na hora; clique direito mostra status HDR ao vivo, pausar atalhos e sair.

## Compilar e instalar para todos os usuários do PC
```powershell
cargo build --release
.\install_all_users.ps1
```
Instalado em `C:\ProgramData\hdr-snip\hdr-snip.exe` com inicialização automática em `C:\ProgramData\Microsoft\Windows\Start Menu\Programs\Startup\HDR Snip.lnk`.
