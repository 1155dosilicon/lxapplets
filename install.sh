#!/usr/bin/env bash
# Instalador dos applets do lxapplets para o lxpanel (qualquer distro Linux).
#
#   ./install.sh               compila, instala e reinicia o painel
#   ./install.sh --uninstall   remove os plugins (e a regra do RAPL, se houver)
#   ./install.sh --help        todas as opções
#
# Detecta a pasta de plugins pelo pkg-config (lxpanel.pc), com alternativas
# para Arch, Fedora, Debian/Ubuntu e openSUSE. Usa `install` (arquivo novo)
# em vez de `cp`: sobrescrever no lugar um .so que o lxpanel tem mapeado na
# memória derruba o painel com SIGSEGV.

set -euo pipefail

PLUGINS=(cilindros)          # crates do workspace que viram plugins
RAPL_RULE=/etc/udev/rules.d/99-rapl-energia.rules
MIN_RUST=1.92

# Caminho absoluto: o pkexec roda o comando a partir de /, então nada de
# caminhos relativos nos comandos executados como root.
ROOT=$(dirname "$(readlink -f "$0")")
cd "$ROOT"

# ---------------------------------------------------------------- saída
if [[ -t 1 ]]; then
    B=$'\e[1m'; G=$'\e[32m'; Y=$'\e[33m'; R=$'\e[31m'; N=$'\e[0m'
else
    B=; G=; Y=; R=; N=
fi
info() { printf '%s==>%s %s\n' "$G$B" "$N" "$*"; }
warn() { printf '%s aviso:%s %s\n' "$Y$B" "$N" "$*" >&2; }
die()  { printf '%s erro:%s %s\n' "$R$B" "$N" "$*" >&2; exit 1; }

usage() {
    cat <<EOF
Uso: $0 [opções]

  (sem opções)    compila em release, instala os plugins e reinicia o lxpanel
  --uninstall     remove os plugins instalados e reinicia o lxpanel
  --no-build      não compila; usa o que já está em target/release
  --no-restart    não reinicia o lxpanel (vale na próxima sessão)
  --rapl          libera os watts da CPU (Intel RAPL) para o seu grupo de admin
  --no-rapl       não pergunta sobre o RAPL
  --plugins-dir D usa a pasta D em vez da detectada
  -h, --help      esta ajuda

Plugins: ${PLUGINS[*]}
EOF
}

# ---------------------------------------------------------------- opções
ACTION=install BUILD=1 RESTART=1 RAPL=ask PLUGINS_DIR=
while (($#)); do
    case $1 in
        --uninstall) ACTION=uninstall ;;
        --no-build) BUILD=0 ;;
        --no-restart) RESTART=0 ;;
        --rapl) RAPL=yes ;;
        --no-rapl) RAPL=no ;;
        --plugins-dir) shift; PLUGINS_DIR=${1:?--plugins-dir precisa de um caminho} ;;
        -h|--help) usage; exit 0 ;;
        *) usage >&2; die "opção desconhecida: $1" ;;
    esac
    shift
done

# ---------------------------------------------------------------- root
if ((EUID == 0)); then
    AS_ROOT=()
elif command -v sudo >/dev/null && { [[ -t 0 ]] || sudo -n true 2>/dev/null; }; then
    AS_ROOT=(sudo)          # com terminal para digitar a senha (ou sem senha)
elif command -v pkexec >/dev/null && [[ -n ${DISPLAY:-}${WAYLAND_DISPLAY:-} ]]; then
    AS_ROOT=(pkexec)        # sem terminal: janela gráfica de senha
elif command -v sudo >/dev/null; then
    AS_ROOT=(sudo)
else
    die "preciso de sudo ou pkexec para escrever na pasta de plugins"
fi

# ---------------------------------------------------------------- distro
distro_hint() {
    local id="" like=""
    if [[ -r /etc/os-release ]]; then
        # shellcheck disable=SC1091
        id=$(. /etc/os-release; echo "${ID:-}")
        # shellcheck disable=SC1091
        like=$(. /etc/os-release; echo "${ID_LIKE:-}")
    fi
    case " $id $like " in
        *" arch "*) echo "sudo pacman -S --needed lxpanel gtk3 pkgconf rustup" ;;
        *" debian "*|*" ubuntu "*)
            echo "sudo apt install lxpanel lxpanel-dev libgtk-3-dev libfm-dev pkg-config build-essential" ;;
        *" fedora "*|*" rhel "*)
            echo "sudo dnf install lxpanel lxpanel-devel gtk3-devel libfm-devel pkgconf gcc" ;;
        *" suse "*|*" opensuse "*)
            echo "sudo zypper install lxpanel lxpanel-devel gtk3-devel libfm-devel pkg-config gcc" ;;
        *) echo "instale o lxpanel com os headers (pacote *-dev/*-devel), GTK 3 e pkg-config" ;;
    esac
}

detect_plugins_dir() {
    [[ -n $PLUGINS_DIR ]] && { echo "$PLUGINS_DIR"; return; }
    local d
    if d=$(pkg-config --variable=pluginsdir lxpanel 2>/dev/null) && [[ -d $d ]]; then
        echo "$d"; return
    fi
    for d in /usr/lib/lxpanel/plugins /usr/lib64/lxpanel/plugins \
             /usr/lib/x86_64-linux-gnu/lxpanel/plugins \
             /usr/lib/aarch64-linux-gnu/lxpanel/plugins \
             /usr/local/lib/lxpanel/plugins; do
        [[ -d $d ]] && { echo "$d"; return; }
    done
    return 1
}

# ---------------------------------------------------------------- painel
PANEL_CMD=()

panel_running() { pgrep -x lxpanel >/dev/null; }

stop_panel() {
    ((RESTART)) || return 0
    panel_running || return 0
    # Guarda a linha de comando para reiniciar igual (perfil incluso).
    read -r -a PANEL_CMD <<<"$(ps -o args= -C lxpanel | head -n1)"
    info "parando o lxpanel"
    # Se algo falhar daqui em diante, o painel é religado antes de sair.
    trap 'start_panel' EXIT
    pkill -x lxpanel || true
    for _ in {1..50}; do panel_running || return 0; sleep 0.1; done
    warn "o lxpanel não parou em 5 s"
}

start_panel() {
    ((RESTART)) || return 0
    ((${#PANEL_CMD[@]})) || return 0   # não estava rodando
    trap - EXIT
    # A sessão (lxsession, entrada "@lxpanel") pode ter reiniciado o painel
    # sozinha enquanto instalávamos; ele pode ter carregado o plugin antigo,
    # então paramos de novo e damos 2 s para a sessão reiniciá-lo.
    if panel_running; then
        pkill -x lxpanel || true
        for _ in {1..20}; do sleep 0.1; panel_running && break; done
    fi
    if ! panel_running; then
        [[ -n ${DISPLAY:-} ]] || { warn "sem DISPLAY; inicie o lxpanel manualmente"; return 0; }
        info "iniciando: ${PANEL_CMD[*]}"
        # setsid -f: sempre cria processo novo e retorna; o painel fica solto
        # (sem ser job deste script, que senão esperaria por ele ao sair).
        (cd ~ && setsid -f "${PANEL_CMD[@]}" >/dev/null 2>&1 </dev/null)
        sleep 1
    fi
    panel_running && info "lxpanel rodando" || warn "o lxpanel não subiu; rode: ${PANEL_CMD[*]}"
}

# ---------------------------------------------------------------- RAPL
admin_group() {
    local g
    for g in wheel sudo admin; do
        if id -nG "$USER" | tr ' ' '\n' | grep -qx "$g"; then echo "$g"; return; fi
    done
    return 1
}

setup_rapl() {
    [[ $RAPL == no ]] && return 0
    [[ -e /sys/class/powercap/intel-rapl:0/energy_uj ]] || return 0
    [[ -r /sys/class/powercap/intel-rapl:0/energy_uj ]] && return 0   # já legível
    [[ -e $RAPL_RULE ]] && return 0
    local group
    group=$(admin_group) || { warn "RAPL: você não está em wheel/sudo/admin; watts da CPU ficam ocultos"; return 0; }
    if [[ $RAPL == ask ]]; then
        [[ -t 0 ]] || return 0
        cat <<EOF

O consumo da CPU em watts vem do contador Intel RAPL, que o kernel deixa
legível só pelo root (desde 2020, por causa do ataque de canal lateral
PLATYPUS, CVE-2020-8694). Posso criar uma regra do udev que libera a leitura
só do contador do pacote da CPU, só para o grupo '$group'.
EOF
        read -r -p "Liberar os watts da CPU? [s/N] " ans
        [[ $ans == [sSyY]* ]] || return 0
    fi
    info "criando $RAPL_RULE (grupo $group)"
    local tmp
    tmp=$(mktemp)
    cat >"$tmp" <<EOF
# Criado pelo instalador do lxapplets: permite que o grupo $group leia o
# contador de energia do pacote da CPU (RAPL), usado pelo plugin "cilindros".
# O kernel deixa energy_uj só para o root desde 2020 (PLATYPUS, CVE-2020-8694);
# aqui só o domínio do pacote (intel-rapl:N, sem subdomínios).
ACTION=="add", SUBSYSTEM=="powercap", KERNEL=="intel-rapl:[0-9]", RUN+="/usr/bin/env chgrp $group /sys%p/energy_uj", RUN+="/usr/bin/env chmod 0440 /sys%p/energy_uj"
EOF
    "${AS_ROOT[@]}" sh -c "install -m644 '$tmp' '$RAPL_RULE' && udevadm control --reload && udevadm trigger --action=add --subsystem-match=powercap && udevadm settle"
    rm -f "$tmp"
}

# ---------------------------------------------------------------- ações
check_build_deps() {
    command -v cargo >/dev/null ||
        die "cargo não encontrado. Instale o Rust: https://rustup.rs (ou: $(distro_hint))"
    pkg-config --exists lxpanel 2>/dev/null ||
        die "lxpanel.pc não encontrado (faltam os headers do lxpanel). Tente: $(distro_hint)"
    pkg-config --exists gtk+-3.0 2>/dev/null ||
        die "GTK 3 (headers) não encontrado. Tente: $(distro_hint)"
    local have
    have=$(rustc --version | awk '{print $2}')
    if [[ $(printf '%s\n%s\n' "$MIN_RUST" "$have" | sort -V | head -n1) != "$MIN_RUST" ]]; then
        die "Rust $have é antigo; preciso de $MIN_RUST ou mais (rode: rustup update)"
    fi
}

do_install() {
    local dir
    dir=$(detect_plugins_dir) || die "pasta de plugins do lxpanel não encontrada; use --plugins-dir. $(distro_hint)"
    if ((BUILD)); then
        check_build_deps
        info "compilando (release): ${PLUGINS[*]}"
        local pkgs=()
        for p in "${PLUGINS[@]}"; do pkgs+=(-p "$p"); done
        cargo build --release --locked "${pkgs[@]}"
    fi
    for p in "${PLUGINS[@]}"; do
        [[ -f $ROOT/target/release/lib$p.so ]] || die "target/release/lib$p.so não existe (rode sem --no-build)"
    done

    stop_panel
    for p in "${PLUGINS[@]}"; do
        info "instalando $dir/$p.so"
        "${AS_ROOT[@]}" install -Dm755 "$ROOT/target/release/lib$p.so" "$dir/$p.so"
    done
    setup_rapl
    start_panel

    for p in "${PLUGINS[@]}"; do
        if ! grep -qs "type=$p" ~/.config/lxpanel/*/panels/* 2>/dev/null; then
            cat <<EOF

${B}$p${N} instalado, mas ainda não está no painel. Para adicionar:
  botão direito no painel → Adicionar/Remover itens do painel → Adicionar → $p
EOF
        fi
    done
    info "pronto"
}

do_uninstall() {
    local dir
    dir=$(detect_plugins_dir) || die "pasta de plugins do lxpanel não encontrada; use --plugins-dir"
    stop_panel
    for p in "${PLUGINS[@]}"; do
        if [[ -e $dir/$p.so ]]; then
            info "removendo $dir/$p.so"
            "${AS_ROOT[@]}" rm -f "$dir/$p.so"
        fi
    done
    if [[ -e $RAPL_RULE ]] && grep -q lxapplets "$RAPL_RULE"; then
        info "removendo $RAPL_RULE (vale no próximo boot)"
        "${AS_ROOT[@]}" rm -f "$RAPL_RULE"
    fi
    start_panel
    info "pronto (a entrada do plugin no painel é ignorada enquanto o .so não existir)"
}

case $ACTION in
    install) do_install ;;
    uninstall) do_uninstall ;;
esac
