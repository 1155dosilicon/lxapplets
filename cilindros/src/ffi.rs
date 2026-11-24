//! Bindings mínimos da API de plugins do lxpanel (ver `/usr/include/lxpanel/plugin.h`).
//!
//! Os símbolos declarados aqui são resolvidos no processo do lxpanel, que já
//! carrega `liblxpanel` (o `build.rs` adiciona o link explícito para que os
//! binários de teste também liguem).

use std::ffi::{c_char, c_int, c_void};

use gtk::{gdk, glib};

#[expect(non_camel_case_types, reason = "nomes espelham a API C do GLib")]
pub type gpointer = *mut c_void;
#[expect(non_camel_case_types, reason = "nomes espelham a API C do GLib")]
pub type gboolean = c_int;

/// Ponteiro para string estática terminada em NUL, usável em `static`.
#[repr(transparent)]
pub struct StaticCStr(pub *const c_char);

// SAFETY: `StaticCStr` só é construído a partir de literais de bytes estáticos
// terminados em NUL (ou nulo). O apontado é imutável e vive todo o processo,
// então compartilhar entre threads é seguro.
unsafe impl Sync for StaticCStr {}

/// Ponteiro `gpointer` estático (nulo), usável em `static`.
#[repr(transparent)]
pub struct StaticVoid(pub gpointer);

// SAFETY: só é construído com nulo; não há dado compartilhado de fato.
unsafe impl Sync for StaticVoid {}

/// Opaque: painel do lxpanel.
#[repr(C)]
pub struct LXPanel {
    _private: [u8; 0],
}

/// Opaque: nó de configuração (API própria do lxpanel, estilo libconfig).
#[repr(C)]
pub struct config_setting_t {
    _private: [u8; 0],
}

/// `GSourceFunc` do GLib.
pub type GSourceFunc = Option<unsafe extern "C" fn(gpointer) -> gboolean>;

/// Descritor do plugin, espelho de `LXPanelPluginInit` de `lxpanel/plugin.h`.
/// Os campos `_reserved*` devem ser nulos; `bitflags` contém os bitfields
/// `one_per_system:1, expand_available:1, expand_default:1, superseded:1`.
#[repr(C)]
pub struct LXPanelPluginInit {
    pub init: Option<extern "C" fn()>,
    pub finalize: Option<extern "C" fn()>,
    pub name: StaticCStr,
    pub description: StaticCStr,
    pub new_instance: Option<
        unsafe extern "C" fn(*mut LXPanel, *mut config_setting_t) -> *mut gtk::ffi::GtkWidget,
    >,
    pub config: Option<
        unsafe extern "C" fn(*mut LXPanel, *mut gtk::ffi::GtkWidget) -> *mut gtk::ffi::GtkWidget,
    >,
    pub reconfigure: Option<unsafe extern "C" fn(*mut LXPanel, *mut gtk::ffi::GtkWidget)>,
    pub button_press_event: Option<
        unsafe extern "C" fn(
            *mut gtk::ffi::GtkWidget,
            *mut gdk::ffi::GdkEventButton,
            *mut LXPanel,
        ) -> gboolean,
    >,
    pub show_system_menu: Option<unsafe extern "C" fn(*mut gtk::ffi::GtkWidget)>,
    pub update_context_menu:
        Option<unsafe extern "C" fn(*mut gtk::ffi::GtkWidget, *mut gtk::ffi::GtkMenu) -> gboolean>,
    pub control: Option<unsafe extern "C" fn(*mut gtk::ffi::GtkWidget, *const c_char) -> gboolean>,
    pub gettext_package: StaticCStr,
    pub _reserved1: StaticVoid,
    pub _reserved2: StaticVoid,
    pub bitflags: c_int,
}

/// Tipos de `PanelConfType` (conf.h): GROUP=0, INT, STRING, LIST.
pub const PANEL_CONF_TYPE_INT: c_int = 1;
pub const PANEL_CONF_TYPE_STRING: c_int = 2;

/// Tipos de `PluginConfType` usados no diálogo genérico (plugin.h). O enum é
/// STR=0, INT, BOOL, FILE, FILE_ENTRY, DIRECTORY_ENTRY, TRIM=6, EXTERNAL=7.
/// (O 6 já esteve aqui como EXTERNAL: o lxpanel então desenhava um rótulo
/// vazio no lugar do botão de intervalo.)
pub const CONF_TYPE_INT: c_int = 1;
pub const CONF_TYPE_BOOL: c_int = 2;
pub const CONF_TYPE_EXTERNAL: c_int = 7;

unsafe extern "C" {
    /// Quark dos dados privados do plugin (plugin.h).
    pub static lxpanel_plugin_qdata: glib::ffi::GQuark;

    pub fn panel_get_orientation(panel: *mut LXPanel) -> c_int;
    pub fn panel_get_height(panel: *mut LXPanel) -> c_int;
    pub fn panel_get_icon_size(panel: *mut LXPanel) -> c_int;
    pub fn lxpanel_config_save(panel: *mut LXPanel);

    pub fn config_setting_lookup_int(
        setting: *const config_setting_t,
        name: *const c_char,
        value: *mut c_int,
    ) -> gboolean;
    pub fn config_setting_lookup_string(
        setting: *const config_setting_t,
        name: *const c_char,
        value: *mut *const c_char,
    ) -> gboolean;
    pub fn config_setting_set_string(
        setting: *mut config_setting_t,
        value: *const c_char,
    ) -> gboolean;
    pub fn config_setting_add(
        parent: *mut config_setting_t,
        name: *const c_char,
        type_: c_int,
    ) -> *mut config_setting_t;
    pub fn config_setting_set_int(setting: *mut config_setting_t, value: c_int) -> gboolean;

    /// Diálogo genérico de configuração; argumentos variáveis são trios
    /// `(nome: *const c_char, valor: gpointer, tipo: c_int)` terminados por NULL.
    pub fn lxpanel_generic_config_dlg(
        title: *const c_char,
        panel: *mut LXPanel,
        apply_func: GSourceFunc,
        plugin: *mut gtk::ffi::GtkWidget,
        ...
    ) -> *mut gtk::ffi::GtkWidget;

    pub fn panel_config_int_button_new(
        name: *const c_char,
        val: *mut c_int,
        min: c_int,
        max: c_int,
    ) -> *mut gtk::ffi::GtkWidget;
}

/// Lê um inteiro da configuração do plugin; devolve `None` se ausente.
///
/// # Safety
/// `conf` deve ser válido até o widget ser destruído (garantia do lxpanel).
pub unsafe fn conf_get_int(conf: *const config_setting_t, name: &std::ffi::CStr) -> Option<c_int> {
    let mut value: c_int = 0;
    // SAFETY: contrato acima; `name` é terminado em NUL por ser `CStr`.
    let ok = unsafe { config_setting_lookup_int(conf, name.as_ptr(), &mut value) };
    (ok != 0).then_some(value)
}

/// Grava um inteiro na configuração do plugin (cria o membro se preciso),
/// no padrão da macro `config_group_set_int` do conf.h.
///
/// # Safety
/// `conf` deve ser válido até o widget ser destruído (garantia do lxpanel).
pub unsafe fn conf_set_int(conf: *mut config_setting_t, name: &std::ffi::CStr, value: c_int) {
    // SAFETY: contrato acima; `config_setting_add` devolve o membro novo ou
    // existente, e `name` é terminado em NUL por ser `CStr`.
    unsafe {
        let member = config_setting_add(conf, name.as_ptr(), PANEL_CONF_TYPE_INT);
        if !member.is_null() {
            config_setting_set_int(member, value);
        }
    }
}

/// Lê um texto da configuração do plugin; devolve `None` se ausente ou se
/// não for UTF-8 válido.
///
/// # Safety
/// `conf` deve ser válido até o widget ser destruído (garantia do lxpanel).
pub unsafe fn conf_get_str(conf: *const config_setting_t, name: &std::ffi::CStr) -> Option<String> {
    let mut value: *const c_char = std::ptr::null();
    // SAFETY: contrato acima; `name` é terminado em NUL por ser `CStr`.
    let ok = unsafe { config_setting_lookup_string(conf, name.as_ptr(), &mut value) };
    if ok == 0 || value.is_null() {
        return None;
    }
    // SAFETY: o lxpanel devolve um ponteiro para string terminada em NUL que
    // vive dentro da configuração; copiamos na hora, antes de qualquer outra
    // chamada que possa alterá-la.
    let s = unsafe { std::ffi::CStr::from_ptr(value) };
    s.to_str().ok().map(str::to_owned)
}

/// Grava um texto na configuração do plugin (padrão de `config_group_set_string`).
/// Texto com NUL interno é ignorado (não há como representá-lo em C).
///
/// # Safety
/// `conf` deve ser válido até o widget ser destruído (garantia do lxpanel).
pub unsafe fn conf_set_str(conf: *mut config_setting_t, name: &std::ffi::CStr, value: &str) {
    let Ok(value) = std::ffi::CString::new(value) else {
        return;
    };
    // SAFETY: contrato acima; nomes e valor são terminados em NUL, e o
    // lxpanel copia o valor (`g_strdup`) em `config_setting_set_string`.
    unsafe {
        let member = config_setting_add(conf, name.as_ptr(), PANEL_CONF_TYPE_STRING);
        if !member.is_null() {
            config_setting_set_string(member, value.as_ptr());
        }
    }
}
