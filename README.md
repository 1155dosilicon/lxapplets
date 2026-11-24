# lxapplets

Applets para o [lxpanel](https://github.com/lxde/lxpanel) (o painel do LXDE),
escritos em Rust.

| Applet | O que faz |
|---|---|
| [**cilindros**](cilindros/) | CPU, GPU, memória, disco e rede como cilindros de vidro com líquido |

![cilindros no painel: CPU, GPU, RAM, IO e NET](docs/cilindros-painel.png)

## Instalar

```sh
git clone https://github.com/1155dosilicon/lxapplets
cd lxapplets
./install.sh
```

O instalador:

1. confere as dependências e diz o que instalar na sua distro se faltar algo;
2. compila em release (`cargo build --release --locked`);
3. acha a pasta de plugins pelo `pkg-config` (`lxpanel.pc`), com alternativas
   para Arch, Fedora, Debian/Ubuntu e openSUSE;
4. para o lxpanel, instala com `install` e o reinicia com o mesmo perfil;
5. pergunta se deve liberar os watts da CPU (regra do udev para o RAPL; veja
   o [README do cilindros](cilindros/README.md)).

Se algo falhar depois de parar o painel, ele é religado antes de o script sair.
Se você usa o `lxsession` com `@lxpanel` no autostart (o padrão do LXDE), o
instalador também lida com o painel que a sessão reinicia sozinha.

Depois, adicione o applet: botão direito no painel → **Adicionar/Remover itens
do painel** → **Adicionar**.

### Opções

```text
./install.sh --uninstall     remove os plugins (e a regra do RAPL criada pelo instalador)
./install.sh --no-build      instala o que já está em target/release
./install.sh --no-restart    não mexe no painel em execução
./install.sh --rapl          libera os watts da CPU sem perguntar
./install.sh --no-rapl       não pergunta sobre o RAPL
./install.sh --plugins-dir D usa a pasta D
```

### Dependências

| Distro | Comando |
|---|---|
| Arch | `sudo pacman -S --needed lxpanel gtk3 pkgconf rustup` |
| Debian/Ubuntu | `sudo apt install lxpanel lxpanel-dev libgtk-3-dev libfm-dev pkg-config build-essential` |
| Fedora | `sudo dnf install lxpanel lxpanel-devel gtk3-devel libfm-devel pkgconf gcc` |
| openSUSE | `sudo zypper install lxpanel lxpanel-devel gtk3-devel libfm-devel pkg-config gcc` |

Rust 1.92 ou mais novo: instale pelo [rustup](https://rustup.rs) se a distro
tiver uma versão antiga.

Só o Arch foi testado de verdade (lxpanel 0.11.1, GTK 3). Nas outras distros
os nomes de pacote e caminhos vêm da documentação delas; relatos são bem-vindos.

## Problemas comuns

- **O painel caiu ao atualizar o plugin.** Não use `cp` por cima de um `.so`
  carregado: ele reescreve o arquivo que o lxpanel tem mapeado na memória e o
  painel cai com SIGSEGV. O instalador usa `install`, que cria um arquivo novo.
  O lxpanel carrega todo `.so` da pasta de plugins, mesmo os que não estão no
  painel.
- **A bandeja do sistema sumiu.** Evite `lxpanelctl restart`; reinicie com
  `pkill -x lxpanel` e inicie de novo (é o que o instalador faz).
- **Os watts da CPU não aparecem.** Rode `./install.sh --rapl`, ou veja a
  seção sobre RAPL no [README do cilindros](cilindros/README.md).
- **A GPU mostra "sem métricas".** Só o driver `amdgpu` expõe o uso no sysfs.

## Estrutura

```text
lxapplets/
├── Cargo.toml        workspace (perfil de release, lints, MSRV)
├── install.sh        instalador
├── cilindros/        applet de medidores
└── docs/             imagens
```

Para um applet novo: crie a crate em uma pasta nova com
`crate-type = ["cdylib", "rlib"]`, adicione-a aos `members` do `Cargo.toml` e
ao array `PLUGINS` do `install.sh`.

## Licença

Copyright © 2026 1155dosilicon. Distribuído sob a **WTFPL**
(*Do What The Fuck You Want To Public License*, versão 2). O texto completo
cabe numa linha e está em [LICENSE](LICENSE):

> 0. You just DO WHAT THE FUCK YOU WANT TO.

Sem garantia nenhuma: se os cilindros transbordarem, o problema é seu.

Os plugins rodam dentro do lxpanel, que é GPL-2.0-or-later; a FSF considera
a WTFPL compatível com a GPL, então dá para distribuir junto sem drama. As
dependências (gtk-rs) são MIT.
