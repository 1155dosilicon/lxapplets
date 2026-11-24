//! Tooltip em markup Pango com os detalhes de cada medidor visível.
//!
//! Os `let _ = write!(s, …)` são de propósito: escrever numa `String` nunca
//! falha (o `fmt::Write` dela sempre devolve `Ok`).

use std::fmt::Write as _;

use crate::gauge::Kind;
use crate::metrics::Metrics;
use gtk::glib;

/// Número com uma casa decimal, vírgula como separador (pt-BR).
fn fmt1(v: f64) -> String {
    format!("{v:.1}").replace('.', ",")
}

/// Percentual inteiro de uma fração 0.0..=1.0.
fn pct(v: f64) -> i64 {
    // Fração limitada a 0..=1: o resultado cabe em i64 sem truncar.
    (v.clamp(0.0, 1.0) * 100.0).round() as i64
}

/// Taxa em bytes/s legível: "512 B/s", "12,3 KB/s", "4,5 MB/s".
pub fn fmt_rate(bps: f64) -> String {
    let bps = bps.max(0.0);
    if bps < 1024.0 {
        format!("{} B/s", bps.round() as i64)
    } else if bps < 1024.0 * 1024.0 {
        format!("{} KB/s", fmt1(bps / 1024.0))
    } else {
        format!("{} MB/s", fmt1(bps / (1024.0 * 1024.0)))
    }
}

/// Tamanho em GiB com uma casa ("123,4 GiB").
fn fmt_gib(bytes: u64) -> String {
    // Perde precisão só acima de 8 PiB; irrelevante para exibir.
    format!("{} GiB", fmt1(bytes as f64 / (1024.0 * 1024.0 * 1024.0)))
}

fn cpu(s: &mut String, m: &Metrics) {
    let _ = write!(s, "<b>CPU</b>  {}%", pct(m.cpu_usage));
    if let Some(w) = m.cpu_watts {
        let _ = write!(s, " · {} W", fmt1(w));
    }
    if let Some(t) = m.cpu_temp_c {
        let _ = write!(s, " · {} °C", fmt1(t));
    }
    if let Some(f) = m.cpu_freq_mhz {
        let _ = write!(s, " · {} MHz", f.round() as i64);
    }
    if !m.cpu_cores.is_empty() {
        let cores: Vec<String> = m
            .cpu_cores
            .iter()
            .take(32)
            .map(|u| format!("{}%", pct(*u)))
            .collect();
        let _ = write!(
            s,
            "\n<small>Núcleos: {}</small>",
            glib::markup_escape_text(&cores.join(" "))
        );
    }
}

fn gpu(s: &mut String, m: &Metrics) {
    let Some(g) = &m.gpu else {
        s.push_str("<b>GPU</b>  <small>sem métricas disponíveis</small>");
        return;
    };
    let _ = write!(
        s,
        "<b>GPU</b> ({})  {}%",
        glib::markup_escape_text(&g.driver),
        pct(g.usage)
    );
    if let Some(w) = g.watts {
        let _ = write!(s, " · {} W", fmt1(w));
    }
    if let Some(t) = g.temp_c {
        let _ = write!(s, " · {} °C", fmt1(t));
    }
    if let Some(f) = g.freq_mhz {
        let _ = write!(s, " · {} MHz", f.round() as i64);
    }
    if let Some(rpm) = g.fan_rpm {
        let _ = write!(s, " · {rpm} RPM");
    }
    if let (Some(used), Some(total)) = (g.vram_used_mib, g.vram_total_mib) {
        let _ = write!(s, "\n<small>VRAM: {used} / {total} MiB</small>");
    }
}

fn mem(s: &mut String, m: &Metrics) {
    let Some(v) = m.mem else {
        s.push_str("<b>RAM</b>  <small>sem dados</small>");
        return;
    };
    let _ = write!(
        s,
        "<b>RAM</b>  {}% · {} / {} MiB",
        pct(v.usage),
        v.used_mib,
        v.total_mib
    );
    if v.swap_total_mib > 0 {
        let _ = write!(
            s,
            "\n<small>Swap: {} / {} MiB</small>",
            v.swap_used_mib, v.swap_total_mib
        );
    }
}

fn disk(s: &mut String, m: &Metrics) {
    let Some(d) = &m.disk else {
        s.push_str("<b>Disco</b>  <small>medindo…</small>");
        return;
    };
    let _ = write!(s, "<b>Disco</b>  {}% ocupado", pct(d.usage));
    if !d.busiest.is_empty() {
        let _ = write!(s, " ({})", glib::markup_escape_text(&d.busiest));
    }
    let _ = write!(
        s,
        " · ↓ {} · ↑ {}",
        fmt_rate(d.read_bps),
        fmt_rate(d.write_bps)
    );
    if let (Some(used), Some(total)) = (d.home_used_bytes, d.home_total_bytes) {
        let frac = if total > 0 {
            // Razão de dois u64: precisão de f64 basta para um percentual.
            used as f64 / total as f64
        } else {
            0.0
        };
        let _ = write!(
            s,
            "\n<small>/home: {} / {} ({}%)</small>",
            fmt_gib(used),
            fmt_gib(total),
            pct(frac)
        );
    }
}

fn net(s: &mut String, m: &Metrics) {
    let Some(n) = m.net else {
        s.push_str("<b>Rede</b>  <small>medindo…</small>");
        return;
    };
    let _ = write!(
        s,
        "<b>Rede</b>  ↓ {} · ↑ {}\n<small>100% = {} (pico recente)</small>",
        fmt_rate(n.rx_bps),
        fmt_rate(n.tx_bps),
        fmt_rate(n.scale_bps)
    );
}

/// Monta o tooltip com as métricas atuais, uma seção por medidor visível.
pub fn build(m: &Metrics, kinds: &[Kind]) -> String {
    let mut s = String::new();
    for (i, k) in kinds.iter().enumerate() {
        if i > 0 {
            s.push('\n');
        }
        match k {
            Kind::Cpu => cpu(&mut s, m),
            Kind::Gpu => gpu(&mut s, m),
            Kind::Mem => mem(&mut s, m),
            Kind::Disk => disk(&mut s, m),
            Kind::Net => net(&mut s, m),
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metrics::{DiskMetrics, GpuMetrics, MemMetrics, NetMetrics};

    #[test]
    fn tooltip_basico() {
        let m = Metrics {
            cpu_usage: 0.234,
            cpu_watts: Some(12.34),
            ..Metrics::default()
        };
        let s = build(&m, &[Kind::Cpu]);
        assert!(s.contains("23%"));
        assert!(s.contains("12,3 W"));
        assert!(!s.contains("GPU"));
    }

    #[test]
    fn tooltip_gpu_ausente() {
        let s = build(&Metrics::default(), &[Kind::Gpu]);
        assert!(s.contains("sem métricas"));
    }

    #[test]
    fn tooltip_escapa_driver() {
        let m = Metrics {
            gpu: Some(GpuMetrics {
                driver: "a<b&c".to_owned(),
                ..GpuMetrics::default()
            }),
            ..Metrics::default()
        };
        let s = build(&m, &[Kind::Gpu]);
        assert!(s.contains("a&lt;b&amp;c"));
    }

    #[test]
    fn tooltip_mem_disco_rede() {
        let m = Metrics {
            mem: Some(MemMetrics {
                usage: 0.5,
                used_mib: 6000,
                total_mib: 12000,
                swap_used_mib: 0,
                swap_total_mib: 0,
            }),
            disk: Some(DiskMetrics {
                usage: 0.1,
                busiest: "sda".to_owned(),
                read_bps: 2.0 * 1024.0 * 1024.0,
                write_bps: 0.0,
                home_used_bytes: Some(50 << 30),
                home_total_bytes: Some(200 << 30),
            }),
            net: Some(NetMetrics {
                usage: 0.3,
                rx_bps: 1536.0,
                tx_bps: 100.0,
                scale_bps: 256.0 * 1024.0,
            }),
            ..Metrics::default()
        };
        let s = build(&m, &[Kind::Mem, Kind::Disk, Kind::Net]);
        assert!(s.contains("6000 / 12000 MiB"), "{s}");
        assert!(!s.contains("Swap"), "sem swap, sem linha: {s}");
        assert!(s.contains("(sda)") && s.contains("↓ 2,0 MB/s"), "{s}");
        assert!(s.contains("/home: 50,0 GiB / 200,0 GiB (25%)"), "{s}");
        assert!(s.contains("↓ 1,5 KB/s · ↑ 100 B/s"), "{s}");
        assert!(s.contains("100% = 256,0 KB/s"), "{s}");
        assert_eq!(s.lines().filter(|l| l.starts_with("<b>")).count(), 3);
    }

    #[test]
    fn taxas() {
        assert_eq!(fmt_rate(0.0), "0 B/s");
        assert_eq!(fmt_rate(-5.0), "0 B/s");
        assert_eq!(fmt_rate(1023.0), "1023 B/s");
        assert_eq!(fmt_rate(1024.0), "1,0 KB/s");
        assert_eq!(fmt_rate(3.0 * 1024.0 * 1024.0), "3,0 MB/s");
    }
}
