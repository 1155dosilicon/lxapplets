//! Teste de fumaça do desenho: renderiza a cena numa superfície de imagem
//! (sem tela) e garante que o Cairo não reporta erro.

use cilindros::gauge::Kind;
use cilindros::render::{self, Item, LabelStyle, Scene};
use gtk::cairo;

fn items(levels: &[f64]) -> Vec<Item> {
    Kind::ALL
        .iter()
        .zip(levels)
        .map(|(k, &level)| Item {
            label: k.label(),
            level,
        })
        .collect()
}

#[test]
fn desenha_sem_erro() -> Result<(), cairo::Error> {
    let surf = cairo::ImageSurface::create(cairo::Format::ARgb32, 120, 40)?;
    let cr = cairo::Context::new(&surf)?;
    let style = LabelStyle::from_pango(LabelStyle::DEFAULT_FONT);
    for levels in [
        [0.0; 5],
        [0.5, 0.6, 0.3, 0.1, 0.9],
        [1.0; 5],
        [-1.0, 2.0, f64::NAN, 0.0, 1.0], // fora da faixa não pode quebrar
    ] {
        let items = items(&levels);
        let scene = Scene {
            horizontal: true,
            items: &items,
            t: 0.3,
            fg: (0.9, 0.9, 0.9),
            show_value: true,
            label: Some(&style),
        };
        render::draw(&cr, 120.0, 40.0, &scene)?;
    }
    // Painel vertical, um cilindro só, e cena vazia.
    for n in [1, 0] {
        let items = items(&[0.0; 5][..n]);
        let scene = Scene {
            horizontal: false,
            items: &items,
            t: 0.0,
            fg: (0.2, 0.2, 0.2),
            show_value: false,
            label: None,
        };
        render::draw(&cr, 30.0, 120.0, &scene)?;
    }
    Ok(())
}
