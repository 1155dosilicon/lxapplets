//! Desenho dos cilindros de vidro com líquido, em Cairo.
//!
//! Cada cilindro é um tubo com tampas elípticas; o nível do líquido representa
//! o uso (0.0..=1.0), a cor vai de verde a vermelho conforme a carga, e há
//! bolhas subindo e um brilho de vidro por cima.

use gtk::cairo;
use pangocairo::pango;

/// Retângulo de desenho em coordenadas do widget.
#[derive(Debug, Clone, Copy)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// Um cilindro a desenhar: rótulo e nível (0.0..=1.0).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Item {
    pub label: &'static str,
    pub level: f64,
}

/// Parâmetros compartilhados da cena.
#[derive(Debug, Clone, Copy)]
pub struct Scene<'a> {
    /// Painel horizontal (cilindros lado a lado) ou vertical (empilhados).
    pub horizontal: bool,
    /// Cilindros, na ordem em que aparecem.
    pub items: &'a [Item],
    /// Tempo em segundos, para animar as bolhas.
    pub t: f64,
    /// Cor do texto dos rótulos (cor do tema do painel).
    pub fg: (f64, f64, f64),
    /// Escreve o percentual no meio do cilindro; sem ele, só o líquido.
    pub show_value: bool,
    /// Rótulo ("CPU", "NET"...) embaixo de cada cilindro, nesta fonte.
    pub label: Option<&'a LabelStyle>,
}

/// Espaço entre cilindros.
const GAP: f64 = 5.0;

/// Fonte dos rótulos, desenhada com Pango (respeita família, peso, largura
/// "Condensed", itálico etc., como o seletor de fonte do GTK descreve).
#[derive(Debug, Clone, PartialEq)]
pub struct LabelStyle {
    desc: pango::FontDescription,
    /// Tamanho em pixels.
    pub size_px: f64,
}

impl LabelStyle {
    /// Fonte padrão: estreita e em negrito, legível em 6–7 px.
    pub const DEFAULT_FONT: &'static str = "DejaVu Sans Condensed Bold 5";

    /// Lê uma descrição de fonte do Pango ("Família Estilo Tamanho"), como
    /// a que o seletor de fonte do GTK devolve. Tamanho em pontos vira pixels
    /// a 96 dpi; tamanhos absurdos são limitados a 4..=16 px.
    pub fn from_pango(desc: &str) -> Self {
        let mut fd = pango::FontDescription::from_string(desc);
        if fd.family().is_none_or(|f| f.is_empty()) {
            fd.set_family("Sans");
        }
        let units = f64::from(fd.size()) / f64::from(pango::SCALE);
        let px = if fd.is_size_absolute() {
            units
        } else {
            units * 96.0 / 72.0
        };
        let size_px = if px > 0.0 { px.clamp(4.0, 16.0) } else { 6.5 };
        fd.set_absolute_size(size_px * f64::from(pango::SCALE));
        Self { desc: fd, size_px }
    }

    /// Família da fonte (sem estilo nem tamanho).
    pub fn family(&self) -> String {
        self.desc.family().map(|f| f.to_string()).unwrap_or_default()
    }

    /// Descrição completa (família, estilo e tamanho).
    pub fn description(&self) -> &pango::FontDescription {
        &self.desc
    }

    /// Altura reservada embaixo do cilindro.
    pub fn height(&self) -> f64 {
        (self.size_px * 1.1).ceil()
    }

    fn layout(&self, cr: &cairo::Context, text: &str, size_px: f64) -> pango::Layout {
        let layout = pangocairo::functions::create_layout(cr);
        let mut desc = self.desc.clone();
        desc.set_absolute_size(size_px * f64::from(pango::SCALE));
        layout.set_font_description(Some(&desc));
        layout.set_text(text);
        layout
    }

    /// Largura do texto nesta fonte, em pixels (para dimensionar o plugin).
    pub fn text_width(&self, text: &str) -> Result<f64, cairo::Error> {
        let surf = cairo::ImageSurface::create(cairo::Format::A8, 1, 1)?;
        let cr = cairo::Context::new(&surf)?;
        let (_, logical) = self.layout(&cr, text, self.size_px).pixel_extents();
        Ok(f64::from(logical.width()))
    }
}

/// Cor do líquido conforme a carga: verde -> âmbar -> vermelho.
pub fn load_color(level: f64) -> (f64, f64, f64) {
    let green = (0.10, 0.72, 0.35);
    let amber = (0.95, 0.72, 0.10);
    let red = (0.90, 0.20, 0.15);
    let lerp = |a: (f64, f64, f64), b: (f64, f64, f64), t: f64| {
        (
            a.0 + (b.0 - a.0) * t,
            a.1 + (b.1 - a.1) * t,
            a.2 + (b.2 - a.2) * t,
        )
    };
    let level = level.clamp(0.0, 1.0);
    if level <= 0.6 {
        lerp(green, amber, level / 0.6)
    } else {
        lerp(amber, red, (level - 0.6) / 0.4)
    }
}

/// Caminho do contorno do tubo (laterais + tampas elípticas).
fn tube_path(
    cr: &cairo::Context,
    cx: f64,
    top: f64,
    bottom: f64,
    rx: f64,
    ry: f64,
) -> Result<(), cairo::Error> {
    let squash = ry / rx;
    cr.move_to(cx - rx, top);
    cr.line_to(cx - rx, bottom);
    cr.save()?;
    cr.translate(cx, bottom);
    cr.scale(1.0, squash);
    cr.arc_negative(0.0, 0.0, rx, std::f64::consts::PI, 0.0);
    cr.restore()?;
    cr.line_to(cx + rx, top);
    cr.save()?;
    cr.translate(cx, top);
    cr.scale(1.0, squash);
    cr.arc_negative(0.0, 0.0, rx, 0.0, -std::f64::consts::PI);
    cr.restore()?;
    cr.close_path();
    Ok(())
}

/// Elipse cheia (tampa ou superfície do líquido).
fn ellipse(cr: &cairo::Context, cx: f64, cy: f64, rx: f64, ry: f64) -> Result<(), cairo::Error> {
    cr.save()?;
    cr.translate(cx, cy);
    cr.scale(1.0, ry / rx);
    cr.arc(0.0, 0.0, rx, 0.0, 2.0 * std::f64::consts::PI);
    cr.restore()?;
    Ok(())
}

/// Desenha um cilindro no retângulo dado. `level` é o uso 0..1 e `label` o
/// texto no topo (omitido se a altura for pequena).
pub fn draw_cylinder(
    cr: &cairo::Context,
    rect: Rect,
    level: f64,
    label: &str,
    scene: &Scene<'_>,
) -> Result<(), cairo::Error> {
    let Rect { x, y, w, h } = rect;
    // `clamp` deixa NaN passar, e NaN em coordenada põe o Cairo em erro
    // (InvalidMatrix) pelo resto do quadro: valor inválido desenha vazio.
    let level = if level.is_finite() {
        level.clamp(0.0, 1.0)
    } else {
        0.0
    };
    // Rótulo embaixo do cilindro, na fonte escolhida.
    let label_h = scene.label.map_or(0.0, LabelStyle::height);
    if let Some(style) = scene.label
        && h > label_h + 8.0
    {
        let mut layout = style.layout(cr, label, style.size_px);
        let (_, mut logical) = layout.pixel_extents();
        // Pode invadir um pouco o espaço entre cilindros; além disso, encolhe.
        let max_w = w + GAP - 1.0;
        let text_w = f64::from(logical.width());
        if text_w > max_w {
            layout = style.layout(cr, label, style.size_px * max_w / text_w);
            (_, logical) = layout.pixel_extents();
        }
        let fg = scene.fg;
        cr.set_source_rgba(fg.0, fg.1, fg.2, 0.9);
        // Centralizado na faixa do rótulo (o Pango posiciona pelo topo).
        cr.move_to(
            x + (w - f64::from(logical.width())) / 2.0 - f64::from(logical.x()),
            y + h - label_h + (label_h - f64::from(logical.height())) / 2.0,
        );
        pangocairo::functions::show_layout(cr, &layout);
    }
    let h = h - label_h;
    let rx = w / 2.0 - 1.0;
    let ry = (w * 0.20).min(h * 0.14).max(2.0);
    let cx = x + w / 2.0;
    let top = y + ry;
    let bottom = y + h - ry;
    let (r, g, b) = load_color(level);

    // Interior do tubo (recorte para o líquido).
    tube_path(cr, cx, top, bottom, rx - 0.8, ry * 0.9)?;
    cr.save()?;
    cr.clip();

    // Líquido: corpo + superfície elípticas.
    let depth = (bottom - top).max(0.0);
    let surface = bottom - level * depth;
    let grad = cairo::LinearGradient::new(0.0, surface, 0.0, bottom + ry);
    grad.add_color_stop_rgba(0.0, r, g, b, 0.95);
    grad.add_color_stop_rgba(1.0, r * 0.55, g * 0.55, b * 0.55, 0.95);
    cr.set_source(&grad)?;
    cr.rectangle(cx - rx, surface, 2.0 * rx, bottom + ry - surface + 1.0);
    cr.fill()?;

    // Superfície (menisco) um pouco mais clara.
    ellipse(cr, cx, surface, rx - 0.8, ry * 0.9)?;
    cr.set_source_rgba(
        (r + 0.35).min(1.0),
        (g + 0.35).min(1.0),
        (b + 0.35).min(1.0),
        0.95,
    );
    cr.fill()?;

    // Bolhas subindo, só quando há líquido suficiente.
    if bottom - surface > 6.0 {
        for i in 0..3u8 {
            let phase = f64::from(i) * 0.37;
            let progress = (scene.t * 0.30 + phase) % 1.0;
            let by = bottom - progress * (bottom - surface);
            let bx = cx + (scene.t * 1.3 + phase * 17.0).sin() * rx * 0.45;
            let br = 0.8 + 0.5 * f64::from(i % 2);
            cr.arc(bx, by, br, 0.0, 2.0 * std::f64::consts::PI);
            cr.set_source_rgba(1.0, 1.0, 1.0, 0.30);
            cr.fill()?;
        }
    }
    cr.restore()?;

    // Vidro: sombreamento horizontal + contorno.
    tube_path(cr, cx, top, bottom, rx, ry)?;
    let glass = cairo::LinearGradient::new(cx - rx, 0.0, cx + rx, 0.0);
    glass.add_color_stop_rgba(0.0, 1.0, 1.0, 1.0, 0.20);
    glass.add_color_stop_rgba(0.35, 1.0, 1.0, 1.0, 0.03);
    glass.add_color_stop_rgba(1.0, 0.0, 0.0, 0.0, 0.12);
    cr.set_source(&glass)?;
    cr.fill_preserve()?;
    cr.set_source_rgba(0.72, 0.78, 0.82, 0.85);
    cr.set_line_width(1.0);
    cr.stroke()?;

    // Faixa de brilho do vidro.
    cr.save()?;
    tube_path(cr, cx, top, bottom, rx - 0.8, ry * 0.9)?;
    cr.clip();
    cr.rectangle(cx - rx * 0.62, top + 1.5, rx * 0.26, depth - 3.0);
    cr.set_source_rgba(1.0, 1.0, 1.0, 0.22);
    cr.fill()?;
    cr.restore()?;

    // Aro superior.
    ellipse(cr, cx, top, rx, ry)?;
    cr.set_source_rgba(1.0, 1.0, 1.0, 0.07);
    cr.fill_preserve()?;
    cr.set_source_rgba(0.85, 0.90, 0.95, 0.8);
    cr.set_line_width(1.0);
    cr.stroke()?;

    // Percentual no centro, com contorno para ler sobre qualquer cor.
    if !scene.show_value {
        return Ok(());
    }
    let text = format!("{}%", (level * 100.0).round() as i32);
    cr.select_font_face("Sans", cairo::FontSlant::Normal, cairo::FontWeight::Bold);
    cr.set_font_size((rx * 0.85).clamp(7.0, 12.0));
    let ext = cr.text_extents(&text)?;
    let tx = cx - ext.width() / 2.0 - ext.x_bearing();
    let ty = (top + bottom) / 2.0 - ext.height() / 2.0 - ext.y_bearing();
    cr.move_to(tx, ty);
    cr.text_path(&text);
    cr.set_source_rgba(0.0, 0.0, 0.0, 0.55);
    cr.set_line_width(2.0);
    cr.stroke_preserve()?;
    cr.set_source_rgba(1.0, 1.0, 1.0, 0.95);
    cr.fill()?;
    Ok(())
}

/// Retângulo do `i`-ésimo de `n` cilindros, lado a lado (horizontal) ou
/// empilhados (vertical), com margem `PAD` e espaço `GAP` entre eles.
pub fn slot(i: usize, n: usize, width: f64, height: f64, horizontal: bool) -> Rect {
    const PAD: f64 = 3.0;
    // Contagens pequenas (até 5 medidores): conversão exata.
    let (i, n) = (i as f64, n.max(1) as f64);
    if horizontal {
        let cw = (width - 2.0 * PAD - (n - 1.0) * GAP) / n;
        Rect {
            x: PAD + i * (cw + GAP),
            y: PAD,
            w: cw,
            h: height - 2.0 * PAD,
        }
    } else {
        let ch = (height - 2.0 * PAD - (n - 1.0) * GAP) / n;
        Rect {
            x: PAD,
            y: PAD + i * (ch + GAP),
            w: width - 2.0 * PAD,
            h: ch,
        }
    }
}

/// Desenha o conjunto de cilindros na alocação do widget.
pub fn draw(
    cr: &cairo::Context,
    width: f64,
    height: f64,
    scene: &Scene<'_>,
) -> Result<(), cairo::Error> {
    let n = scene.items.len();
    for (i, item) in scene.items.iter().enumerate() {
        let rect = slot(i, n, width, height, scene.horizontal);
        if rect.w > 4.0 && rect.h > 8.0 {
            draw_cylinder(cr, rect, item.level, item.label, scene)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cor_nos_extremos() {
        let (r, g, b) = load_color(0.0);
        assert!(g > r && g > b); // verde
        let (r, g, b) = load_color(1.0);
        assert!(r > g && r > b); // vermelho
    }

    #[test]
    fn cor_continua() {
        // Sem saltos entre os segmentos da interpolação.
        let a = load_color(0.6 - 1e-9);
        let b = load_color(0.6 + 1e-9);
        assert!((a.0 - b.0).abs() < 1e-6);
        assert!((a.1 - b.1).abs() < 1e-6);
    }

    #[test]
    fn slots_nao_se_sobrepoem() {
        for horizontal in [true, false] {
            let rects: Vec<Rect> = (0..5)
                .map(|i| slot(i, 5, 111.0, 111.0, horizontal))
                .collect();
            for w in rects.windows(2) {
                let (a, b) = (w[0], w[1]);
                if horizontal {
                    assert!(a.x + a.w < b.x, "{a:?} {b:?}");
                } else {
                    assert!(a.y + a.h < b.y, "{a:?} {b:?}");
                }
            }
            let last = rects[4];
            assert!(last.x + last.w <= 111.0 && last.y + last.h <= 111.0);
        }
    }

    #[test]
    fn cor_clamp() {
        assert_eq!(load_color(-1.0), load_color(0.0));
        assert_eq!(load_color(2.0), load_color(1.0));
    }

    #[test]
    fn fonte_do_pango() {
        // "Condensed" é largura (stretch) para o Pango, não parte da família:
        // precisa sobreviver até o desenho.
        let s = LabelStyle::from_pango("DejaVu Sans Condensed Bold 6");
        assert_eq!(s.family(), "DejaVu Sans");
        assert_eq!(s.description().stretch(), pango::Stretch::Condensed);
        assert_eq!(s.description().weight(), pango::Weight::Bold);
        assert!((s.size_px - 8.0).abs() < 1e-9, "6 pt = 8 px a 96 dpi");

        let s = LabelStyle::from_pango("Liberation Sans Italic 30px");
        assert_eq!(s.description().style(), pango::Style::Italic);
        assert_eq!(s.size_px, 16.0, "limitado a 16 px");

        // Sem tamanho nem família: padrão legível.
        let s = LabelStyle::from_pango("");
        assert_eq!(s.family(), "Sans");
        assert_eq!(s.size_px, 6.5);
    }

    #[test]
    fn largura_do_rotulo() -> Result<(), cairo::Error> {
        let s = LabelStyle::from_pango(LabelStyle::DEFAULT_FONT);
        let curto = s.text_width("IO")?;
        let longo = s.text_width("REDE")?;
        assert!(curto > 0.0 && longo > curto, "{curto} {longo}");
        Ok(())
    }
}
