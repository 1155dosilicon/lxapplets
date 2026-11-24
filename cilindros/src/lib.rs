// Plugin do lxpanel "Cilindros": medidores de CPU, GPU, memória, disco e rede
// em forma de cilindros de vidro com líquido, desenhados com Cairo.
//
// O lxpanel carrega plugins como módulos libfm: os três símbolos exportados
// no fim deste arquivo (`module_name`, `module_lxpanel_gtk_version` e
// `fm_module_init_lxpanel_gtk`) são o ponto de entrada. Todo o resto é Rust
// seguro; os callbacks FFI capturam pânicos para não derrubar o painel.

pub mod ffi;
pub mod gauge;
pub mod metrics;
pub mod render;
pub mod tooltip;

use std::cell::{Cell, RefCell};
use std::ffi::{c_char, c_int};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::rc::Rc;
use std::time::{Duration, Instant};

use glib::translate::{Borrowed, from_glib_borrow};
use gtk::prelude::*;
use gtk::{gdk, glib};

use ffi::{gboolean, gpointer};
use gauge::{COUNT, Kind, Visible};

/// Preferências do plugin (persistidas na configuração do painel).
struct Settings {
    interval_ms: u32,
    visible: Visible,
    animate: bool,
    /// Percentual escrito no cilindro (padrão: desligado, só o líquido).
    show_value: bool,
    /// Rótulos embaixo dos cilindros (padrão: ligado).
    labels: bool,
    /// Fonte dos rótulos, como o seletor de fonte do GTK descreve
    /// ("DejaVu Sans Condensed Bold 5"), e a mesma já interpretada.
    font: String,
    label_style: render::LabelStyle,
}

impl Settings {
    /// Estilo do rótulo, se os rótulos estiverem ligados.
    fn label(&self) -> Option<&render::LabelStyle> {
        self.labels.then_some(&self.label_style)
    }
}

/// Estado de uma instância do plugin no painel. Vive na thread principal;
/// os timers seguram apenas `Weak`, e o `Rc` forte fica no qdata do widget.
struct Instance {
    area: glib::WeakRef<gtk::DrawingArea>,
    sampler: RefCell<metrics::Sampler>,
    metrics: RefCell<metrics::Metrics>,
    /// Níveis exibidos (interpolados em direção ao uso real), por
    /// [`Kind::index`].
    disp: [Cell<f64>; COUNT],
    horizontal: Cell<bool>,
    thickness: Cell<i32>,
    settings: RefCell<Settings>,
    /// Ponteiros garantidos pelo lxpanel até o widget ser destruído.
    conf: *mut ffi::config_setting_t,
    panel: *mut ffi::LXPanel,
    data_source: Cell<Option<glib::SourceId>>,
    anim_source: Cell<Option<glib::SourceId>>,
    /// Timer rápido que acumula leituras de uso da GPU (ver `GPU_POLL`).
    gpu_source: Cell<Option<glib::SourceId>>,
    /// `Weak` para a própria instância, usado pelos timers.
    weak_self: std::cell::OnceCell<std::rc::Weak<Instance>>,
    /// Valores temporários do diálogo de configuração (o diálogo genérico
    /// guarda ponteiros, então precisam viver na instância, não na pilha).
    cfg_interval: Cell<c_int>,
    cfg_show: [Cell<c_int>; COUNT],
    cfg_anim: Cell<c_int>,
    cfg_value: Cell<c_int>,
    cfg_labels: Cell<c_int>,
    start: Instant,
    /// Contador de quadros da animação (para pular quadros ociosos).
    frame: Cell<u32>,
}

impl Instance {
    /// Remove todos os timers ativos.
    fn stop_timers(&self) {
        for cell in [&self.data_source, &self.anim_source, &self.gpu_source] {
            if let Some(id) = cell.take() {
                id.remove();
            }
        }
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        self.stop_timers();
    }
}

/// Período das leituras rápidas de uso da GPU. O contador do amdgpu é
/// instantâneo (ver `metrics::BusyAverager`); 50 Hz dá ~50 leituras por
/// segundo de intervalo, a ~2 µs cada.
const GPU_POLL: Duration = Duration::from_millis(20);

/// Executa `f` capturando pânicos: um pânico cruzando a fronteira FFI
/// abortaria o processo do lxpanel, então devolvemos `fallback` em vez disso.
fn guarded<R>(f: impl FnOnce() -> R, fallback: R) -> R {
    catch_unwind(AssertUnwindSafe(f)).unwrap_or(fallback)
}

/// Acessa a instância anexada ao widget via qdata do lxpanel.
fn with_instance<R>(widget: *mut gtk::ffi::GtkWidget, f: impl FnOnce(&Instance) -> R) -> Option<R> {
    if widget.is_null() {
        return None;
    }
    // SAFETY: `widget` é o GtkEventBox criado em `new_instance`; o lxpanel
    // garante que ele está vivo durante os callbacks. `g_object_get_qdata`
    // só lê o ponteiro guardado, sem mexer na contagem de referências.
    let ptr =
        unsafe { glib::gobject_ffi::g_object_get_qdata(widget.cast(), ffi::lxpanel_plugin_qdata) };
    if ptr.is_null() {
        return None;
    }
    // SAFETY: o qdata guarda um `Rc<Instance>` vivo (ver acima); a referência
    // compartilhada não conflita com o `Rc` (nenhum `&mut` é criado).
    Some(f(unsafe { &*(ptr as *const Instance) }))
}

/// Tamanho pedido ao painel conforme orientação, espessura e quantidade de
/// cilindros.
fn layout_size(thickness: i32, horizontal: bool, s: &Settings) -> (i32, i32) {
    let kinds = s.visible.kinds();
    // No máximo `COUNT` (5) medidores: cabe em i32.
    let n = i32::try_from(kinds.len()).unwrap_or(1);
    let t = thickness.max(16);
    let label = s.label();
    if horizontal {
        // O rótulo pode invadir até 4 px do espaço entre cilindros (ver
        // `render::draw_cylinder`); se for mais largo, alarga o cilindro.
        let label_w = label.map_or(0.0, |style| {
            kinds
                .iter()
                // Falha de medição (sem memória para a superfície 1x1) só
                // deixa o rótulo encolher no desenho.
                .map(|k| style.text_width(k.label()).unwrap_or(0.0))
                .fold(0.0, f64::max)
        });
        // f64 -> i32 após clamp em 14..40: sem truncamento perigoso.
        let cw = (f64::from(t) * 0.62)
            .max(label_w.ceil() - 4.0)
            .clamp(14.0, 40.0) as i32;
        (n * cw + (n - 1) * 5 + 6, t)
    } else {
        // Altura do rótulo limitada a 4..=18 px: cabe em i32.
        let label_h = label.map_or(0, |style| style.height() as i32);
        let ch = t * 3 / 2 + label_h;
        (t, n * ch + (n - 1) * 5 + 6)
    }
}

/// Recalcula o tamanho pedido ao painel e redesenha.
fn relayout(inst: &Instance) {
    let (w, h) = layout_size(
        inst.thickness.get(),
        inst.horizontal.get(),
        &inst.settings.borrow(),
    );
    if let Some(area) = inst.area.upgrade() {
        area.set_size_request(w, h);
        area.queue_draw();
    }
}

/// Amostra as métricas e atualiza tooltip e desenho.
fn tick_data(inst: &Instance) {
    let m = inst.sampler.borrow_mut().sample();
    let animate = inst.settings.borrow().animate;
    if !animate {
        for k in Kind::ALL {
            inst.disp[k.index()].set(k.level(&m));
        }
    }
    *inst.metrics.borrow_mut() = m;
    if let Some(area) = inst.area.upgrade() {
        let kinds = inst.settings.borrow().visible.kinds();
        let text = tooltip::build(&inst.metrics.borrow(), &kinds);
        area.set_tooltip_markup(Some(&text));
        area.queue_draw();
    }
}

/// Período da animação. Quase todo o custo do plugin é o Cairo desenhando
/// (medido com perf: ~2/3 do tempo do lxpanel a 30 fps), então a taxa de
/// quadros é o que decide o consumo.
const ANIM_FRAME: Duration = Duration::from_millis(66);

/// Com os níveis parados, redesenha só a cada N quadros (~7,5 fps): basta
/// para as bolhas, que sobem ~6 px/s.
const IDLE_FRAME_DIV: u32 = 2;

/// Aproxima o nível exibido do alvo. 44% da distância por quadro de 66 ms dá
/// a mesma constante de tempo de 25% a cada 33 ms (0,75² ≈ 0,56).
/// Para a menos de meio ponto percentual: o texto mostra inteiros.
fn ease(cur: f64, target: f64) -> f64 {
    let d = target - cur;
    if d.abs() < 0.004 {
        target
    } else {
        cur + d * 0.44
    }
}

/// Interpola os níveis exibidos e redesenha (também anima as bolhas).
fn tick_anim(inst: &Instance) {
    let mut moving = false;
    {
        let m = inst.metrics.borrow();
        let visible = inst.settings.borrow().visible;
        for k in Kind::ALL {
            let cell = &inst.disp[k.index()];
            let (old, new) = (cell.get(), ease(cell.get(), k.level(&m)));
            cell.set(new);
            // Ocultos também acompanham o alvo, mas não pedem redesenho.
            moving |= visible.get(k) && new != old;
        }
    }
    let frame = inst.frame.get().wrapping_add(1);
    inst.frame.set(frame);
    if !moving && !frame.is_multiple_of(IDLE_FRAME_DIV) {
        return;
    }
    if let Some(area) = inst.area.upgrade() {
        area.queue_draw();
    }
}

fn start_data_timer(inst: &Instance) {
    let Some(weak) = inst.weak_self.get().cloned() else {
        return; // weak_self ainda não inicializado; timers só depois
    };
    let interval = Duration::from_millis(u64::from(inst.settings.borrow().interval_ms));
    let id = glib::timeout_add_local(interval, move || {
        let Some(inst) = weak.upgrade() else {
            return glib::ControlFlow::Break;
        };
        tick_data(&inst);
        glib::ControlFlow::Continue
    });
    inst.data_source.set(Some(id));
}

fn start_anim_timer(inst: &Instance) {
    if !inst.settings.borrow().animate {
        return;
    }
    let Some(weak) = inst.weak_self.get().cloned() else {
        return;
    };
    let id = glib::timeout_add_local(ANIM_FRAME, move || {
        let Some(inst) = weak.upgrade() else {
            return glib::ControlFlow::Break;
        };
        tick_anim(&inst);
        glib::ControlFlow::Continue
    });
    inst.anim_source.set(Some(id));
}

fn start_gpu_timer(inst: &Instance) {
    if !inst.settings.borrow().visible.get(Kind::Gpu) || !inst.sampler.borrow_mut().has_gpu() {
        return; // sem GPU com métricas, não acorda 50 vezes por segundo à toa
    }
    let Some(weak) = inst.weak_self.get().cloned() else {
        return;
    };
    let id = glib::timeout_add_local(GPU_POLL, move || {
        let Some(inst) = weak.upgrade() else {
            return glib::ControlFlow::Break;
        };
        inst.sampler.borrow_mut().poll_gpu();
        glib::ControlFlow::Continue
    });
    inst.gpu_source.set(Some(id));
}

fn start_timers(inst: &Instance) {
    start_gpu_timer(inst);
    start_data_timer(inst);
    start_anim_timer(inst);
}

fn restart_timers(inst: &Instance) {
    inst.stop_timers();
    start_timers(inst);
}

/// Converte o intervalo da configuração (já limitado a 250..=5000) em ms.
fn interval_ms(v: c_int) -> u32 {
    u32::try_from(v.clamp(250, 5000)).unwrap_or(1000)
}

/// Callback `new_instance`: cria o widget e anexa o estado.
extern "C" fn new_instance(
    panel: *mut ffi::LXPanel,
    conf: *mut ffi::config_setting_t,
) -> *mut gtk::ffi::GtkWidget {
    guarded(|| new_instance_inner(panel, conf), std::ptr::null_mut())
}

fn new_instance_inner(
    panel: *mut ffi::LXPanel,
    conf: *mut ffi::config_setting_t,
) -> *mut gtk::ffi::GtkWidget {
    if !gtk::is_initialized() && gtk::init().is_err() {
        return std::ptr::null_mut();
    }

    // SAFETY: `conf` e `panel` são válidos durante todo o callback (garantia
    // do lxpanel); os nomes são `CStr` estáticos.
    let (interval, animate, show_value, horizontal, thickness) = unsafe {
        (
            ffi::conf_get_int(conf, c"interval")
                .unwrap_or(1000)
                .clamp(250, 5000),
            ffi::conf_get_int(conf, c"animate").unwrap_or(1) != 0,
            ffi::conf_get_int(conf, c"value").unwrap_or(0) != 0,
            ffi::panel_get_orientation(panel) == 0,
            ffi::panel_get_height(panel),
        )
    };
    // Medidor sem chave na configuração começa ligado.
    // SAFETY: como acima; `conf_key` devolve `CStr` estático.
    let visible = Visible::from_flags(
        Kind::ALL.map(|k| unsafe { ffi::conf_get_int(conf, k.conf_key()) }.unwrap_or(1) != 0),
    );
    // SAFETY: como acima.
    let (labels, font) = unsafe {
        (
            ffi::conf_get_int(conf, c"labels").unwrap_or(1) != 0,
            ffi::conf_get_str(conf, c"font")
                .filter(|f| !f.trim().is_empty())
                .unwrap_or_else(|| render::LabelStyle::DEFAULT_FONT.to_owned()),
        )
    };

    // O widget devolvido ao lxpanel precisa ser um container (ele chama
    // `gtk_container_set_border_width` nele) e receber cliques (ele liga o
    // menu do botão direito em "button-press-event"). Um GtkDrawingArea sozinho
    // não é nenhum dos dois, então o desenho vai dentro de um GtkEventBox.
    //
    // O event box é criado cru para preservar a referência flutuante: o
    // lxpanel assume a posse ao empacotar no painel. `from_glib_borrow`
    // empresta sem ref/sink, então ele segue flutuante ao ser devolvido.
    // SAFETY: GTK inicializado acima; `gtk_event_box_new` devolve widget
    // flutuante vivo ou nulo.
    let raw = unsafe { gtk::ffi::gtk_event_box_new() };
    if raw.is_null() {
        return std::ptr::null_mut();
    }
    // SAFETY: `raw` é não nulo (checado acima) e aponta para widget flutuante
    // vivo; o empréstimo não altera a contagem de referências.
    let ebox: Borrowed<gtk::EventBox> = unsafe { from_glib_borrow(raw.cast()) };
    // Sem janela visível: herda o fundo do painel, mas ainda capta eventos.
    ebox.set_visible_window(false);
    let area = gtk::DrawingArea::new();
    // O GtkDrawingArea tem janela X própria; sem esta máscara o X entrega o
    // clique direto à janela do painel, que abre o menu do painel em vez do
    // menu do plugin (sem "Configurações"/"Remover"). Com ela o GTK recebe o
    // clique na área e o propaga ao event box, onde o lxpanel ligou o menu.
    area.add_events(gdk::EventMask::BUTTON_PRESS_MASK | gdk::EventMask::BUTTON_RELEASE_MASK);
    ebox.add(&area);
    area.show();

    let inst = Rc::new(Instance {
        area: {
            let w = glib::WeakRef::<gtk::DrawingArea>::new();
            w.set(Some(&area));
            w
        },
        sampler: RefCell::new(metrics::Sampler::new()),
        metrics: RefCell::new(metrics::Metrics::default()),
        disp: Default::default(),
        horizontal: Cell::new(horizontal),
        thickness: Cell::new(thickness),
        settings: RefCell::new(Settings {
            interval_ms: interval_ms(interval),
            visible,
            animate,
            show_value,
            labels,
            label_style: render::LabelStyle::from_pango(&font),
            font,
        }),
        conf,
        panel,
        data_source: Cell::new(None),
        anim_source: Cell::new(None),
        gpu_source: Cell::new(None),
        weak_self: std::cell::OnceCell::new(),
        cfg_interval: Cell::new(interval),
        cfg_show: Kind::ALL.map(|k| Cell::new(c_int::from(visible.get(k)))),
        cfg_anim: Cell::new(c_int::from(animate)),
        cfg_value: Cell::new(c_int::from(show_value)),
        cfg_labels: Cell::new(c_int::from(labels)),
        start: Instant::now(),
        frame: Cell::new(0),
    });
    // Recém-criado, o `OnceCell` está vazio: o `set` não falha.
    let _ = inst.weak_self.set(Rc::downgrade(&inst));

    area.set_has_tooltip(true);
    relayout(&inst);

    let weak = Rc::downgrade(&inst);
    area.connect_draw(move |widget, cr| {
        let Some(inst) = weak.upgrade() else {
            return glib::Propagation::Proceed;
        };
        let rgba = widget.style_context().color(gtk::StateFlags::NORMAL);
        let settings = inst.settings.borrow();
        let items: Vec<render::Item> = settings
            .visible
            .kinds()
            .into_iter()
            .map(|k| render::Item {
                label: k.label(),
                level: inst.disp[k.index()].get(),
            })
            .collect();
        let scene = render::Scene {
            horizontal: inst.horizontal.get(),
            items: &items,
            t: inst.start.elapsed().as_secs_f64(),
            fg: (rgba.red(), rgba.green(), rgba.blue()),
            show_value: settings.show_value,
            label: settings.label(),
        };
        let res = render::draw(
            cr,
            f64::from(widget.allocated_width()),
            f64::from(widget.allocated_height()),
            &scene,
        );
        // Erro de desenho (ex.: superfície inválida) não tem recuperação
        // dentro do callback draw; ignoramos para não derrubar o painel.
        let _ = res;
        glib::Propagation::Proceed
    });

    // O qdata fica no event box, que é o widget que o lxpanel passa de volta
    // a `reconfigure`, `config` e `apply_config`.
    // SAFETY: `ebox` é um GObject vivo; o qdata recebe o `Rc` forte da
    // instância, e `destroy_data` o libera exatamente uma vez no dispose.
    unsafe {
        glib::gobject_ffi::g_object_set_qdata_full(
            ebox.as_ptr().cast(),
            ffi::lxpanel_plugin_qdata,
            Rc::into_raw(Rc::clone(&inst)).cast_mut().cast(),
            Some(destroy_data),
        );
    }

    start_timers(&inst);
    tick_data(&inst);
    raw
}

/// Destrutor do qdata: libera o `Rc` da instância (e os timers, via `Drop`).
///
/// # Safety
/// `data` deve ser o ponteiro criado por `Rc::into_raw` em `new_instance_inner`;
/// o GObject o chama uma única vez, no dispose do widget.
unsafe extern "C" fn destroy_data(data: gpointer) {
    // SAFETY: contrato acima; `data` é não nulo (o GObject só chama o
    // destrutor para dados presentes).
    let inst = unsafe { Rc::<Instance>::from_raw(data.cast()) };
    // O `Drop` remove timers (`SourceId::remove` entra em pânico se a fonte
    // já sumiu); um pânico aqui abortaria o lxpanel inteiro.
    guarded(move || drop(inst), ());
}

/// Callback `reconfigure`: a geometria do painel mudou.
extern "C" fn reconfigure(panel: *mut ffi::LXPanel, widget: *mut gtk::ffi::GtkWidget) {
    guarded(
        || {
            with_instance(widget, |inst| {
                // SAFETY: `panel` válido durante o callback (garantia do lxpanel).
                let (horizontal, thickness) = unsafe {
                    (
                        ffi::panel_get_orientation(panel),
                        ffi::panel_get_height(panel),
                    )
                };
                let horizontal = horizontal == 0;
                inst.horizontal.set(horizontal);
                inst.thickness.set(thickness);
                relayout(inst);
            });
        },
        (),
    );
}

/// Callback `config`: diálogo com intervalo, medidores visíveis e animação.
extern "C" fn config(
    panel: *mut ffi::LXPanel,
    widget: *mut gtk::ffi::GtkWidget,
) -> *mut gtk::ffi::GtkWidget {
    guarded(|| config_inner(panel, widget), std::ptr::null_mut())
}

fn config_inner(
    panel: *mut ffi::LXPanel,
    widget: *mut gtk::ffi::GtkWidget,
) -> *mut gtk::ffi::GtkWidget {
    with_instance(widget, |inst| {
        {
            let s = inst.settings.borrow();
            inst.cfg_interval
                .set(c_int::try_from(s.interval_ms).unwrap_or(1000));
            for k in Kind::ALL {
                inst.cfg_show[k.index()].set(c_int::from(s.visible.get(k)));
            }
            inst.cfg_anim.set(c_int::from(s.animate));
            inst.cfg_value.set(c_int::from(s.show_value));
            inst.cfg_labels.set(c_int::from(s.labels));
        }
        let show = |k: Kind| inst.cfg_show[k.index()].as_ptr();
        let font_row = font_chooser_row(inst);
        // SAFETY: ponteiros válidos durante o callback; os valores apontados
        // vivem na instância até o widget ser destruído, e o diálogo é
        // destruído antes disso (garantia do lxpanel).
        unsafe {
            let int_button = ffi::panel_config_int_button_new(
                c"Intervalo (ms)".as_ptr(),
                inst.cfg_interval.as_ptr(),
                250,
                5000,
            );
            ffi::lxpanel_generic_config_dlg(
                c"Cilindros".as_ptr(),
                panel,
                Some(apply_config),
                widget,
                c"".as_ptr(),
                int_button,
                ffi::CONF_TYPE_EXTERNAL,
                // Um trio (rótulo, ponteiro, tipo) por medidor, na ordem de
                // `Kind::ALL`. Escrito por extenso porque é varargs de C.
                Kind::Cpu.conf_label().as_ptr(),
                show(Kind::Cpu),
                ffi::CONF_TYPE_BOOL,
                Kind::Gpu.conf_label().as_ptr(),
                show(Kind::Gpu),
                ffi::CONF_TYPE_BOOL,
                Kind::Mem.conf_label().as_ptr(),
                show(Kind::Mem),
                ffi::CONF_TYPE_BOOL,
                Kind::Disk.conf_label().as_ptr(),
                show(Kind::Disk),
                ffi::CONF_TYPE_BOOL,
                Kind::Net.conf_label().as_ptr(),
                show(Kind::Net),
                ffi::CONF_TYPE_BOOL,
                c"Animar líquido".as_ptr(),
                inst.cfg_anim.as_ptr(),
                ffi::CONF_TYPE_BOOL,
                c"Mostrar percentual nos cilindros".as_ptr(),
                inst.cfg_value.as_ptr(),
                ffi::CONF_TYPE_BOOL,
                c"Mostrar rótulos (CPU, GPU…)".as_ptr(),
                inst.cfg_labels.as_ptr(),
                ffi::CONF_TYPE_BOOL,
                c"".as_ptr(),
                // O diálogo empacota o widget (ganha a própria referência);
                // a nossa cai quando `font_row` sai de escopo.
                font_row.as_ptr().cast::<gtk::ffi::GtkWidget>(),
                ffi::CONF_TYPE_EXTERNAL,
                std::ptr::null::<c_char>(),
            )
        }
    })
    .unwrap_or(std::ptr::null_mut())
}

/// Linha "Fonte dos rótulos: [seletor]" do diálogo. A fonte escolhida vale na
/// hora (o diálogo genérico só conhece os próprios tipos, então o seletor
/// grava a configuração sozinho).
fn font_chooser_row(inst: &Instance) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let button = gtk::FontButton::with_font(&inst.settings.borrow().font);
    button.set_use_font(true);
    button.set_title("Fonte dos rótulos");
    row.pack_start(&gtk::Label::new(Some("Fonte dos rótulos")), false, false, 2);
    row.pack_start(&button, true, true, 2);
    row.show_all();

    let weak = inst.weak_self.get().cloned();
    button.connect_font_set(move |b| {
        let Some(inst) = weak.as_ref().and_then(std::rc::Weak::upgrade) else {
            return;
        };
        let Some(font) = b.font() else {
            return;
        };
        {
            let mut s = inst.settings.borrow_mut();
            s.label_style = render::LabelStyle::from_pango(&font);
            s.font = font.to_string();
        }
        // SAFETY: `conf` e `panel` valem até o widget ser destruído
        // (garantia do lxpanel), e o diálogo morre antes dele.
        unsafe {
            ffi::conf_set_str(inst.conf, c"font", &font);
            ffi::lxpanel_config_save(inst.panel);
        }
        relayout(&inst);
    });
    row
}

/// Aplicação do diálogo de configuração (chamada com o widget como dado).
///
/// # Safety
/// `data` é o ponteiro do widget passado a `lxpanel_generic_config_dlg`.
unsafe extern "C" fn apply_config(data: gpointer) -> gboolean {
    guarded(
        || {
            with_instance(data.cast(), |inst| {
                let interval = inst.cfg_interval.get().clamp(250, 5000);
                let visible =
                    Visible::from_flags(Kind::ALL.map(|k| inst.cfg_show[k.index()].get() != 0));
                let animate = inst.cfg_anim.get() != 0;
                let show_value = inst.cfg_value.get() != 0;
                let labels = inst.cfg_labels.get() != 0;
                {
                    let mut s = inst.settings.borrow_mut();
                    s.interval_ms = interval_ms(interval);
                    s.visible = visible;
                    s.animate = animate;
                    s.show_value = show_value;
                    s.labels = labels;
                }
                // SAFETY: `conf` é válido até o widget ser destruído (garantia
                // do lxpanel); nomes são `CStr` estáticos.
                unsafe {
                    ffi::conf_set_int(inst.conf, c"interval", interval);
                    for k in Kind::ALL {
                        ffi::conf_set_int(inst.conf, k.conf_key(), c_int::from(visible.get(k)));
                    }
                    ffi::conf_set_int(inst.conf, c"animate", c_int::from(animate));
                    ffi::conf_set_int(inst.conf, c"value", c_int::from(show_value));
                    ffi::conf_set_int(inst.conf, c"labels", c_int::from(labels));
                    // `panel` vive enquanto a instância existe.
                    ffi::lxpanel_config_save(inst.panel);
                }
                restart_timers(inst);
                relayout(inst);
            });
        },
        (),
    );
    1
}

/// Clique esquerdo abre o gerenciador de tarefas (melhor esforço).
extern "C" fn button_press_event(
    _widget: *mut gtk::ffi::GtkWidget,
    event: *mut gdk::ffi::GdkEventButton,
    _panel: *mut ffi::LXPanel,
) -> gboolean {
    guarded(
        || {
            if event.is_null() {
                return;
            }
            // SAFETY: o lxpanel passa um evento válido durante o callback.
            let button = unsafe { (*event).button };
            if button == 1 {
                // Lançamento melhor esforço: se `lxtask` não existir, o erro
                // é ignorado de propósito (sem diálogo de erro no painel).
                // SAFETY: string estática terminada em NUL; `error` nulo é
                // permitido por `g_spawn_command_line_async`.
                unsafe {
                    glib::ffi::g_spawn_command_line_async(c"lxtask".as_ptr(), std::ptr::null_mut());
                }
            }
        },
        (),
    );
    0
}

// ---------------------------------------------------------------------------
// Ponto de entrada do módulo libfm (ver FM_DEFINE_MODULE em libfm/fm-module.h)
// ---------------------------------------------------------------------------

/// Versão da API `lxpanel_gtk` (FM_MODULE_lxpanel_gtk_VERSION).
#[unsafe(no_mangle)]
pub static module_lxpanel_gtk_version: c_int = 1;

/// Nome do tipo do plugin, como aparece na configuração do painel.
#[unsafe(no_mangle)]
pub static module_name: [u8; 10] = *b"cilindros\0";

/// Descritor do plugin.
#[unsafe(no_mangle)]
pub static fm_module_init_lxpanel_gtk: ffi::LXPanelPluginInit = ffi::LXPanelPluginInit {
    init: None,
    finalize: None,
    name: ffi::StaticCStr(c"Cilindros".as_ptr()),
    description: ffi::StaticCStr(
        c"CPU, GPU, memória, disco e rede em cilindros com líquido".as_ptr(),
    ),
    new_instance: Some(new_instance),
    config: Some(config),
    reconfigure: Some(reconfigure),
    button_press_event: Some(button_press_event),
    show_system_menu: None,
    update_context_menu: None,
    control: None,
    gettext_package: ffi::StaticCStr(std::ptr::null()),
    _reserved1: ffi::StaticVoid(std::ptr::null_mut()),
    _reserved2: ffi::StaticVoid(std::ptr::null_mut()),
    bitflags: 1, // one_per_system = 1
};
