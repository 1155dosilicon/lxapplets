// Liga o plugin contra a liblxpanel presente no sistema. Em produção os
// símbolos já estão no processo do lxpanel; o link explícito é necessário
// para os binários de teste (`cargo test`) resolverem as declarações FFI.
//
// A liblxpanel fica num diretório privado que muda entre distros
// (`/usr/lib/lxpanel` no Arch e Fedora, `/usr/lib/x86_64-linux-gnu/lxpanel`
// no Debian/Ubuntu). Ordem de busca:
//   1. variável de ambiente `LXPANEL_LIBDIR`;
//   2. `pkg-config --variable=pkglibdir lxpanel`;
//   3. diretórios conhecidos.

use std::path::{Path, PathBuf};
use std::process::Command;

fn has_lib(dir: &Path) -> bool {
    dir.join("liblxpanel.so").exists() || dir.join("liblxpanel.so.0").exists()
}

fn pkg_config_libdir() -> Option<PathBuf> {
    let out = Command::new("pkg-config")
        .args(["--variable=pkglibdir", "lxpanel"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let dir = String::from_utf8(out.stdout).ok()?;
    let dir = dir.trim();
    (!dir.is_empty()).then(|| PathBuf::from(dir))
}

fn find_libdir() -> PathBuf {
    if let Some(dir) = std::env::var_os("LXPANEL_LIBDIR") {
        return PathBuf::from(dir);
    }
    if let Some(dir) = pkg_config_libdir().filter(|d| has_lib(d)) {
        return dir;
    }
    let candidates = [
        "/usr/lib/lxpanel",
        "/usr/lib64/lxpanel",
        "/usr/lib/x86_64-linux-gnu/lxpanel",
        "/usr/lib/aarch64-linux-gnu/lxpanel",
        "/usr/local/lib/lxpanel",
    ];
    candidates
        .iter()
        .map(PathBuf::from)
        .find(|d| has_lib(d))
        .unwrap_or_else(|| {
            println!(
                "cargo:warning=liblxpanel não encontrada; instale o lxpanel com os \
                 headers (ex.: lxpanel-dev no Debian) ou defina LXPANEL_LIBDIR"
            );
            PathBuf::from("/usr/lib/lxpanel")
        })
}

fn main() {
    let dir = find_libdir();
    println!("cargo:rustc-link-search=native={}", dir.display());
    println!("cargo:rustc-link-lib=dylib=lxpanel");
    // rpath para o diretório privado da lib (fora do caminho padrão do
    // linker); dentro do painel a lib já está carregada, mas isso também
    // permite carregar o módulo fora dele (testes, exemplos).
    println!("cargo:rustc-link-arg=-Wl,-rpath,{}", dir.display());
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=LXPANEL_LIBDIR");
}
