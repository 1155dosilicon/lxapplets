# Cilindros — plugin do lxpanel em Rust

Medidores de **CPU, GPU, memória, disco e rede** em forma de cilindros de vidro
com líquido: o nível sobe com o uso, a cor vai de verde a vermelho conforme a
carga, há bolhas animadas, um rótulo embaixo de cada cilindro e um tooltip com
os detalhes. Ordem no painel: CPU, GPU, RAM, IO, NET.

| Medidor | Fonte dos dados | O que o nível mostra | Tooltip |
|---|---|---|---|
| **CPU** | `/proc/stat` | uso total | watts (Intel RAPL), temperatura (hwmon `coretemp`/`k10temp`), clock médio, uso por núcleo |
| **GPU** | sysfs do `amdgpu` | `gpu_busy_percent` lido a 50 Hz, média do intervalo | watts, temperatura, clock, ventoinha, VRAM |
| **RAM** | `/proc/meminfo` | `MemTotal − MemAvailable` (como o `free`) | usado/total, swap |
| **IO** | `/proc/diskstats` | fração do tempo em que o disco físico mais ocupado fez E/S (o `%util` do `iostat`) | leitura e escrita somadas, espaço do sistema de arquivos do `/home` |
| **NET** | `/proc/net/dev` | tráfego das interfaces físicas relativo ao pico recente | download, upload, valor do 100% |

Detalhes de medição:

- **GPU:** o contador do amdgpu é instantâneo. No SMU7 (Polaris) ele reflete
  ~1 ms de atividade e pula entre 0 e 100. Por isso o plugin lê a cada 20 ms
  (~2 µs por leitura, com o arquivo mantido aberto) e mostra a média.
  GPUs sem esse arquivo (NVIDIA, Intel) aparecem como "sem métricas".
- **CPU (watts):** o `energy_uj` do RAPL é legível só pelo root desde 2020
  (PLATYPUS, CVE-2020-8694). O instalador oferece uma regra do udev que libera
  só o contador do pacote, só para o grupo de administração (`wheel`/`sudo`).
  Sem ela, os watts não aparecem; o resto funciona.
- **IO:** só discos físicos (com `device` no sysfs). Loop, zram, dm e
  partições ficam de fora.
- **NET:** só interfaces físicas (sem `lo`, bridges, veth, tun). Como rede
  não tem um 100% natural, a escala acompanha o pico recente: meia-vida de
  30 s e piso de 256 KB/s, para o tráfego de fundo não parecer 100%.

## Instalar

Da raiz do repositório:

```sh
./install.sh
```

Depois: botão direito no painel → **Adicionar/Remover itens do painel** →
**Adicionar** → **Cilindros**. Detalhes do instalador no
[README principal](../README.md).

## Configurar

Botão direito nos cilindros → **Configurações de "Cilindros"**:

- intervalo de atualização (250–5000 ms);
- quais medidores mostrar (com todos desligados, a CPU continua, para o
  plugin não sumir do painel);
- animação do líquido;
- percentual escrito dentro do cilindro (desligado por padrão);
- rótulos embaixo dos cilindros e a fonte deles (padrão:
  `DejaVu Sans Condensed Bold 5`). A fonte vale na hora.

Clique esquerdo abre o `lxtask`, se estiver instalado.

## Desenvolvimento

```sh
cargo build --release                      # da raiz do workspace
cargo run -p cilindros --example preview   # PNG com níveis e fontes de rótulo
cargo run -p cilindros --example amostra   # texto do tooltip no terminal
```

O `preview` aceita um arquivo de saída e fontes:
`cargo run -p cilindros --example preview /tmp/p.png "Liberation Sans Bold 5"`.

Verificação:

```sh
cargo clippy --workspace --all-targets -- -D warnings
cargo nextest run          # ou cargo test
cargo audit
```

Requisitos de build: Rust 1.92+, `lxpanel` 0.11 com headers, GTK 3, Pango e
`pkg-config`. O `build.rs` acha a `liblxpanel` pelo `pkg-config`
(`pkglibdir`); para forçar outro lugar, use `LXPANEL_LIBDIR=/caminho`.

## Notas de implementação

- **Entrada do módulo:** o lxpanel carrega plugins como módulos libfm. Os
  símbolos `module_name`, `module_lxpanel_gtk_version` e
  `fm_module_init_lxpanel_gtk` ficam no fim de `src/lib.rs`; os bindings
  mínimos da API estão em `src/ffi.rs`.
- **Widget:** o lxpanel chama `gtk_container_set_border_width` no widget do
  plugin e liga o menu do botão direito no `button-press-event` dele. Por
  isso o plugin devolve um `GtkEventBox` (container) com o `GtkDrawingArea`
  dentro, e a área de desenho tem `BUTTON_PRESS_MASK`. Sem essa máscara o X
  entrega o clique à janela do painel, que abre o menu do painel em vez do
  menu do plugin. O event box é criado cru para preservar a referência
  flutuante que o lxpanel assume.
- **Estado:** um `Rc<Instance>` no qdata do event box. Os timers e o callback
  de desenho seguram só `Weak`; o destrutor do qdata remove os timers.
- **Pânicos:** todo callback FFI passa por `guarded` (`catch_unwind`), porque
  um pânico cruzando a fronteira C abortaria o lxpanel. Por isso o perfil de
  release usa `panic = "unwind"`, nunca `"abort"`.
- **Falhas de leitura:** arquivos de `/proc` e sysfs que faltam viram `None`;
  o plugin mostra o que houver.
- **Desempenho:** quase todo o custo é o Cairo desenhando. A animação roda a
  15 fps durante transições e ~7,5 fps parada (só bolhas). Os rótulos usam
  Pango (`pangocairo`) porque a API de texto simples do Cairo ignora largura
  da fonte ("Condensed") e pesos intermediários.
- **Constantes da API do lxpanel** que já deram errado: `CONF_TYPE_EXTERNAL`
  vale 7 (o 6 é `CONF_TYPE_TRIM`).
