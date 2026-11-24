//! Gera uma prévia dos cilindros em PNG, sem precisar do painel:
//! `cargo run --example preview [arquivo.png] ["Fonte 5" ...]`
//! (padrão: /tmp/opencode/cilindros-preview.png e algumas fontes de exemplo).
//!
//! Linha de cima: cilindros grandes em vários níveis. Embaixo: uma faixa de
//! painel em tamanho real (28 px) para cada fonte de rótulo.

use cilindros::gauge::Kind;
use cilindros::render::{self, Item, LabelStyle, Rect, Scene};
use gtk::cairo;

const PANEL_H: f64 = 28.0;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let out = args
        .next()
        .unwrap_or_else(|| "/tmp/opencode/cilindros-preview.png".to_owned());
    let mut fonts: Vec<String> = args.collect();
    if fonts.is_empty() {
        fonts = [
            LabelStyle::DEFAULT_FONT,
            "DejaVu Sans Condensed 5",
            "JetBrains Mono ExtraBold 5",
            "Liberation Sans Bold 5",
        ]
        .map(str::to_owned)
        .to_vec();
    }

    let rows = fonts.len() as f64; // poucas fontes: conversão exata
    let height = 180.0 + rows * (PANEL_H + 8.0);
    // Dimensões pequenas e positivas: cabem em i32.
    let surf = cairo::ImageSurface::create(cairo::Format::ARgb32, 500, height as i32)?;
    let cr = cairo::Context::new(&surf)?;
    cr.set_source_rgba(0.13, 0.14, 0.16, 1.0);
    cr.paint()?;
    let fg = (0.85, 0.85, 0.85);

    // Linha 1: cilindros grandes em vários níveis de carga, com percentual.
    for (i, level) in [0.05, 0.23, 0.42, 0.65, 0.85, 0.98].into_iter().enumerate() {
        let scene = Scene {
            horizontal: true,
            items: &[],
            t: 1.7,
            fg,
            show_value: true,
            label: None,
        };
        let rect = Rect {
            x: 14.0 + i as f64 * 80.0,
            y: 16.0,
            w: 62.0,
            h: 140.0,
        };
        render::draw_cylinder(&cr, rect, level, "CPU", &scene)?;
    }

    // Faixas de painel, uma por fonte.
    let items: Vec<Item> = Kind::ALL
        .iter()
        .zip([0.23, 0.42, 0.66, 0.08, 0.91])
        .map(|(k, level)| Item {
            label: k.label(),
            level,
        })
        .collect();
    for (row, font) in fonts.iter().enumerate() {
        let style = LabelStyle::from_pango(font);
        let y = 176.0 + row as f64 * (PANEL_H + 8.0);
        cr.set_source_rgba(0.18, 0.19, 0.22, 1.0);
        cr.rectangle(0.0, y, 500.0, PANEL_H);
        cr.fill()?;
        // Mesma largura que o plugin pede ao painel.
        let label_w = items
            .iter()
            .map(|it| style.text_width(it.label))
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .fold(0.0, f64::max);
        let cw = 17.0_f64.max(label_w - 3.0);
        let width = 5.0 * cw + 4.0 * 5.0 + 6.0;
        let scene = Scene {
            horizontal: true,
            items: &items,
            t: 1.7,
            fg,
            show_value: false,
            label: Some(&style),
        };
        cr.save()?;
        cr.translate(14.0, y);
        render::draw(&cr, width, PANEL_H, &scene)?;
        cr.restore()?;
        cr.select_font_face("Sans", cairo::FontSlant::Normal, cairo::FontWeight::Normal);
        cr.set_font_size(11.0);
        cr.set_source_rgba(0.7, 0.7, 0.7, 1.0);
        cr.move_to(width + 30.0, y + 18.0);
        cr.show_text(font)?;
    }

    let mut file = std::fs::File::create(&out)?;
    surf.write_to_png(&mut file)?;
    println!("prévia gravada em {out}");
    Ok(())
}
