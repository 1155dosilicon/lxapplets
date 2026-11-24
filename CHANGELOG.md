# Changelog

Formato baseado em [Keep a Changelog](https://keepachangelog.com/pt-BR/1.1.0/).

## [cilindros 0.2.0] — 2026-11-24

### Adicionado
- Medidores de memória (RAM), atividade de disco (IO) e rede (NET), além de
  CPU e GPU; cada um pode ser ligado ou desligado.
- Rótulos embaixo dos cilindros, desenhados com Pango, com fonte escolhível
  no diálogo de configuração.
- Opção de mostrar o percentual dentro do cilindro (desligada por padrão).
- Espaço do `/home` e velocidades de leitura/escrita no tooltip.
- Exemplo `amostra`, que imprime o tooltip no terminal.
- Instalador `install.sh` para qualquer distro, com `--uninstall` e regra
  opcional do udev para os watts da CPU (RAPL).

### Corrigido
- Uso da GPU pulando entre 0 e 100% no amdgpu: agora é a média de leituras
  a 50 Hz.
- Clique direito nos cilindros abria o menu do painel em vez do menu do
  plugin (faltava `BUTTON_PRESS_MASK` na área de desenho).
- A opção "Intervalo (ms)" nunca aparecia no diálogo (`CONF_TYPE_EXTERNAL`
  valia 6 em vez de 7).
- Aviso `gtk_container_set_border_width` no log: o widget do plugin agora é
  um `GtkEventBox`.
- Clock da CPU nunca aparecia (`/proc/cpuinfo` usa tabs antes do `:`).
- Nível `NaN` deixava o Cairo em erro pelo resto do quadro.
- Pânico no destrutor do plugin abortaria o lxpanel.
- MSRV declarado (1.88) era menor que o exigido pela gtk-rs (1.92).

### Desempenho
- Animação de 30 para 15 fps em transições e ~7,5 fps parada: o consumo do
  lxpanel caiu de ~7,8% para ~2% de um núcleo.
- Perfil de release com LTO, `codegen-units = 1` e strip (`.so` de 678 para
  ~490 KB).

## [cilindros 0.1.0]

- Versão inicial: medidores de CPU e GPU.
