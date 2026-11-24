//! Imprime no terminal o mesmo texto do tooltip, sem o painel:
//! `cargo run --example amostra [segundos]` (padrão: 1 s entre amostras).
//! Útil para conferir as métricas desta máquina.

use std::time::{Duration, Instant};

use cilindros::gauge::Kind;
use cilindros::metrics::Sampler;
use cilindros::tooltip;

fn main() {
    let secs: f64 = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(1.0);
    let mut sampler = Sampler::new();
    // Primeira amostra só guarda os contadores (sem delta ainda).
    sampler.sample();
    // Como no plugin: leituras da GPU a 50 Hz durante o intervalo.
    let end = Instant::now() + Duration::from_secs_f64(secs.max(0.1));
    while Instant::now() < end {
        sampler.poll_gpu();
        std::thread::sleep(Duration::from_millis(20));
    }
    let m = sampler.sample();
    // Tira o markup Pango para ler no terminal.
    let text = tooltip::build(&m, &Kind::ALL);
    let plain = text
        .replace("<b>", "")
        .replace("</b>", "")
        .replace("<small>", "  ")
        .replace("</small>", "")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&");
    println!("{plain}");
}
