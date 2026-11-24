//! Coleta de métricas de CPU e GPU lendo `/proc` e sysfs (sem dependências).
//!
//! CPU: uso por `/proc/stat`, watts por Intel RAPL (`powercap`), temperatura
//! por hwmon (`coretemp`/`k10temp`) e clock por `/proc/cpuinfo`.
//! GPU (amdgpu): `gpu_busy_percent`, VRAM, e hwmon com watts, temperatura,
//! clock e ventoinha. Toda leitura falha de forma silenciosa (`None`), pois
//! o plugin roda dentro do processo do painel.

use std::fs;
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// Contadores de jiffies agregados de uma linha `cpu` do `/proc/stat`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Jiffies {
    pub total: u64,
    pub idle: u64,
}

/// Métricas de um instante.
#[derive(Debug, Default, Clone)]
pub struct Metrics {
    /// Uso total da CPU, 0.0..=1.0.
    pub cpu_usage: f64,
    pub cpu_watts: Option<f64>,
    pub cpu_temp_c: Option<f64>,
    pub cpu_freq_mhz: Option<f64>,
    /// Uso por núcleo, 0.0..=1.0.
    pub cpu_cores: Vec<f64>,
    pub gpu: Option<GpuMetrics>,
    pub mem: Option<MemMetrics>,
    pub disk: Option<DiskMetrics>,
    pub net: Option<NetMetrics>,
}

/// Memória RAM e swap, de `/proc/meminfo`.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct MemMetrics {
    /// RAM em uso (`MemTotal - MemAvailable`, como o `free`), 0.0..=1.0.
    pub usage: f64,
    pub used_mib: u64,
    pub total_mib: u64,
    pub swap_used_mib: u64,
    pub swap_total_mib: u64,
}

/// Atividade dos discos físicos, de `/proc/diskstats`.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct DiskMetrics {
    /// Fração do tempo em que o disco mais ocupado esteve fazendo E/S
    /// (o `%util` do `iostat`), 0.0..=1.0.
    pub usage: f64,
    /// Disco mais ocupado no intervalo.
    pub busiest: String,
    /// Leitura e escrita somadas de todos os discos físicos.
    pub read_bps: f64,
    pub write_bps: f64,
    /// Espaço do sistema de arquivos que contém o `/home`.
    pub home_used_bytes: Option<u64>,
    pub home_total_bytes: Option<u64>,
}

/// Tráfego das interfaces de rede físicas, de `/proc/net/dev`.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct NetMetrics {
    /// Tráfego total relativo ao pico recente (ver [`AdaptiveScale`]), 0.0..=1.0.
    pub usage: f64,
    pub rx_bps: f64,
    pub tx_bps: f64,
    /// Valor que corresponde a 100% no momento.
    pub scale_bps: f64,
}

/// Interpreta o `/proc/meminfo`. Devolve `None` sem `MemTotal`/`MemAvailable`.
pub fn parse_meminfo(text: &str) -> Option<MemMetrics> {
    let mut total = None;
    let mut avail = None;
    let mut swap_total = 0u64;
    let mut swap_free = 0u64;
    for line in text.lines() {
        let Some((key, rest)) = line.split_once(':') else {
            continue;
        };
        // Valores em kB: "MemTotal:       12176768 kB".
        let Some(kb) = rest
            .split_whitespace()
            .next()
            .and_then(|v| v.parse::<u64>().ok())
        else {
            continue;
        };
        match key {
            "MemTotal" => total = Some(kb),
            "MemAvailable" => avail = Some(kb),
            "SwapTotal" => swap_total = kb,
            "SwapFree" => swap_free = kb,
            _ => {}
        }
    }
    let total = total.filter(|&t| t > 0)?;
    let used = total.saturating_sub(avail?);
    Some(MemMetrics {
        // kB cabem com folga na mantissa do f64 (2^53 kB = 8 EiB).
        usage: (used as f64 / total as f64).clamp(0.0, 1.0),
        used_mib: used / 1024,
        total_mib: total / 1024,
        swap_used_mib: swap_total.saturating_sub(swap_free) / 1024,
        swap_total_mib: swap_total / 1024,
    })
}

/// Contadores de um disco no `/proc/diskstats`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiskCounters {
    pub name: String,
    pub sectors_read: u64,
    pub sectors_written: u64,
    /// Milissegundos gastos fazendo E/S (campo `io_ticks`).
    pub io_ticks_ms: u64,
}

/// Interpreta o `/proc/diskstats`, aceitando só os nomes que `keep` aprovar.
///
/// Formato: `major minor nome` seguidos dos contadores; o 3º é setores lidos,
/// o 7º setores escritos e o 10º `io_ticks` (Documentation/admin-guide/iostats.rst).
pub fn parse_diskstats(text: &str, keep: impl Fn(&str) -> bool) -> Vec<DiskCounters> {
    let mut out = Vec::new();
    for line in text.lines() {
        let mut it = line.split_whitespace();
        let (Some(_major), Some(_minor), Some(name)) = (it.next(), it.next(), it.next()) else {
            continue;
        };
        if !keep(name) {
            continue;
        }
        let vals: Vec<u64> = it.take(10).filter_map(|v| v.parse().ok()).collect();
        if vals.len() < 10 {
            continue;
        }
        out.push(DiskCounters {
            name: name.to_owned(),
            sectors_read: vals[2],
            sectors_written: vals[6],
            io_ticks_ms: vals[9],
        });
    }
    out
}

/// Bytes recebidos e enviados de uma interface no `/proc/net/dev`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetCounters {
    pub name: String,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
}

/// Interpreta o `/proc/net/dev`, aceitando só as interfaces que `keep` aprovar.
/// Formato: `nome: rx_bytes rx_pacotes ... (8 campos) tx_bytes ...`.
pub fn parse_net_dev(text: &str, keep: impl Fn(&str) -> bool) -> Vec<NetCounters> {
    let mut out = Vec::new();
    for line in text.lines() {
        // As duas linhas de cabeçalho não têm ':' depois do nome.
        let Some((name, rest)) = line.split_once(':') else {
            continue;
        };
        let name = name.trim();
        if name.contains('|') || !keep(name) {
            continue;
        }
        let vals: Vec<u64> = rest
            .split_whitespace()
            .take(9)
            .filter_map(|v| v.parse().ok())
            .collect();
        if vals.len() < 9 {
            continue;
        }
        out.push(NetCounters {
            name: name.to_owned(),
            rx_bytes: vals[0],
            tx_bytes: vals[8],
        });
    }
    out
}

/// Taxa por segundo entre dois contadores monotônicos. Se o contador voltou
/// (interface reiniciada, disco removido e recolocado), devolve 0.
pub fn rate(prev: u64, cur: u64, dt_secs: f64) -> f64 {
    if dt_secs <= 0.0 {
        return 0.0;
    }
    // Deltas de um intervalo cabem na mantissa do f64.
    cur.saturating_sub(prev) as f64 / dt_secs
}

/// Escala que acompanha o pico recente, para medidores sem um "100%" natural
/// (como a rede). O pico sobe na hora e decai pela metade a cada
/// [`AdaptiveScale::HALF_LIFE_SECS`], sem cair abaixo de um piso: assim um
/// download grande enche o cilindro, e o tráfego ocioso não parece 100%.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AdaptiveScale {
    peak: f64,
    floor: f64,
}

impl AdaptiveScale {
    pub const HALF_LIFE_SECS: f64 = 30.0;

    pub fn new(floor: f64) -> Self {
        Self { peak: floor, floor }
    }

    /// Registra um valor e devolve (fração 0.0..=1.0, valor do 100%).
    pub fn update(&mut self, value: f64, dt_secs: f64) -> (f64, f64) {
        let decay = 0.5f64.powf(dt_secs.max(0.0) / Self::HALF_LIFE_SECS);
        self.peak = (self.peak * decay).max(value).max(self.floor);
        ((value / self.peak).clamp(0.0, 1.0), self.peak)
    }
}

/// Disco físico: tem `device` no sysfs (exclui loop, zram, dm, md, partições).
fn is_physical_disk(name: &str) -> bool {
    !name.contains('/') && Path::new("/sys/block").join(name).join("device").exists()
}

/// Interface física: tem `device` no sysfs (exclui lo, bridges, veth, tun).
fn is_physical_iface(name: &str) -> bool {
    !name.contains('/')
        && Path::new("/sys/class/net")
            .join(name)
            .join("device")
            .exists()
}

/// Espaço (usado, total) em bytes do sistema de arquivos que contém `path`.
fn fs_space(path: &str) -> Option<(u64, u64)> {
    use gtk::gio;
    use gtk::gio::prelude::*;
    let info = gio::File::for_path(path)
        .query_filesystem_info(
            "filesystem::size,filesystem::used",
            None::<&gio::Cancellable>,
        )
        .ok()?;
    // `used` é o mesmo do `df` (blocos totais menos livres).
    let total = info.attribute_uint64("filesystem::size");
    let used = info.attribute_uint64("filesystem::used");
    (total > 0).then_some((used.min(total), total))
}

/// Calcula as métricas de disco entre duas leituras do `/proc/diskstats`.
pub fn disk_metrics(prev: &[DiskCounters], cur: &[DiskCounters], dt_secs: f64) -> DiskMetrics {
    let mut d = DiskMetrics::default();
    if dt_secs <= 0.0 {
        return d;
    }
    for c in cur {
        // Disco novo (sem leitura anterior) entra só na próxima amostra.
        let Some(p) = prev.iter().find(|p| p.name == c.name) else {
            continue;
        };
        let util = (rate(p.io_ticks_ms, c.io_ticks_ms, dt_secs) / 1000.0).clamp(0.0, 1.0);
        if util > d.usage || d.busiest.is_empty() {
            d.usage = util;
            d.busiest.clone_from(&c.name);
        }
        // Setores do diskstats são sempre de 512 bytes, qualquer que seja o disco.
        d.read_bps += rate(p.sectors_read, c.sectors_read, dt_secs) * 512.0;
        d.write_bps += rate(p.sectors_written, c.sectors_written, dt_secs) * 512.0;
    }
    d
}

/// Soma (rx, tx) em bytes/s entre duas leituras do `/proc/net/dev`.
pub fn net_rates(prev: &[NetCounters], cur: &[NetCounters], dt_secs: f64) -> (f64, f64) {
    let mut rx = 0.0;
    let mut tx = 0.0;
    for c in cur {
        if let Some(p) = prev.iter().find(|p| p.name == c.name) {
            rx += rate(p.rx_bytes, c.rx_bytes, dt_secs);
            tx += rate(p.tx_bytes, c.tx_bytes, dt_secs);
        }
    }
    (rx, tx)
}

/// Piso da escala da rede: abaixo de 256 KiB/s o cilindro não enche.
const NET_SCALE_FLOOR_BPS: f64 = 256.0 * 1024.0;

/// Métricas da GPU.
#[derive(Debug, Default, Clone)]
pub struct GpuMetrics {
    /// Uso do motor gráfico, 0.0..=1.0.
    pub usage: f64,
    pub watts: Option<f64>,
    pub temp_c: Option<f64>,
    pub freq_mhz: Option<f64>,
    pub fan_rpm: Option<u32>,
    pub vram_used_mib: Option<u64>,
    pub vram_total_mib: Option<u64>,
    pub driver: String,
}

/// Interpreta o `/proc/stat`: devolve os contadores agregados e por núcleo.
/// `idle` inclui `iowait`; `total` soma os 8 primeiros campos (guest já está
/// contabilizado em user/nice).
pub fn parse_proc_stat(text: &str) -> Option<(Jiffies, Vec<Jiffies>)> {
    fn parse_line(line: &str) -> Option<Jiffies> {
        let mut it = line.split_whitespace().skip(1); // pula "cpu"/"cpuN"
        let mut vals = [0u64; 8];
        for v in &mut vals {
            *v = it.next()?.parse().ok()?;
        }
        let total: u64 = vals.iter().sum();
        Some(Jiffies {
            total,
            idle: vals[3].saturating_add(vals[4]),
        })
    }

    let mut agg = None;
    let mut cores = Vec::new();
    for line in text.lines() {
        if line.starts_with("cpu ") {
            agg = parse_line(line);
        } else if line.starts_with("cpu") {
            if let Some(j) = parse_line(line) {
                cores.push(j);
            }
        } else if agg.is_some() {
            break; // passou das linhas de CPU
        }
    }
    Some((agg?, cores))
}

/// Uso (0.0..=1.0) entre duas amostras de jiffies.
pub fn usage_from_delta(prev: Jiffies, cur: Jiffies) -> f64 {
    let dt = cur.total.saturating_sub(prev.total);
    let di = cur.idle.saturating_sub(prev.idle);
    if dt == 0 {
        return 0.0;
    }
    // Conversão u64 -> f64 perde precisão acima de 2^53 jiffies (~1 milhão de
    // anos de CPU); aceitável para uma razão de diferenças pequenas.
    ((dt - di) as f64 / dt as f64).clamp(0.0, 1.0)
}

/// Watts a partir do delta do contador RAPL, tratando o wrap do contador.
pub fn rapl_watts(
    prev_energy_uj: u64,
    cur_energy_uj: u64,
    max_range_uj: u64,
    dt_secs: f64,
) -> Option<f64> {
    if dt_secs <= 0.0 || max_range_uj == 0 {
        return None;
    }
    let delta = if cur_energy_uj >= prev_energy_uj {
        cur_energy_uj - prev_energy_uj
    } else {
        max_range_uj
            .checked_sub(prev_energy_uj)?
            .checked_add(cur_energy_uj)?
    };
    Some(delta as f64 / 1e6 / dt_secs)
}

fn read_trim(path: &Path) -> Option<String> {
    fs::read_to_string(path).ok().map(|s| s.trim().to_owned())
}

fn read_u64(path: &Path) -> Option<u64> {
    read_trim(path)?.parse().ok()
}

fn read_f64(path: &Path) -> Option<f64> {
    read_trim(path)?.parse().ok()
}

/// Estado do contador RAPL entre amostras.
struct Rapl {
    energy_path: PathBuf,
    max_range_uj: u64,
    prev_energy_uj: u64,
    prev_time: Instant,
}

/// Interpreta o conteúdo de `gpu_busy_percent` ("42\n") como fração 0.0..=1.0.
pub fn parse_busy_percent(bytes: &[u8]) -> Option<f64> {
    let text = std::str::from_utf8(bytes).ok()?.trim();
    let pct: u32 = text.parse().ok()?;
    Some((f64::from(pct) / 100.0).clamp(0.0, 1.0))
}

/// Média das leituras de uso da GPU acumuladas entre duas amostras.
///
/// O `gpu_busy_percent` do amdgpu é instantâneo: no SMU7 (Polaris) ele reflete
/// a atividade do último ~1 ms, então sob carga leve alterna entre 0 e picos,
/// e mesmo a 100% cai a 0 por alguns ms entre quadros. Uma leitura por segundo
/// pega um ponto aleatório dessa onda. Lendo várias vezes por intervalo
/// (ver [`Sampler::poll_gpu`]) e tirando a média, o valor converge para a
/// fração de tempo em que a GPU realmente esteve ocupada.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct BusyAverager {
    sum: f64,
    count: u32,
}

impl BusyAverager {
    /// Acumula uma leitura (0.0..=1.0).
    pub fn push(&mut self, usage: f64) {
        self.sum += usage.clamp(0.0, 1.0);
        self.count = self.count.saturating_add(1);
    }

    /// Devolve a média acumulada e zera; `None` se não houve leitura.
    pub fn take(&mut self) -> Option<f64> {
        let avg = (self.count > 0).then(|| self.sum / f64::from(self.count));
        *self = Self::default();
        avg
    }
}

/// Caminhos da GPU descobertos uma única vez.
struct GpuPaths {
    device: PathBuf,
    hwmon: Option<PathBuf>,
    driver: String,
    /// `gpu_busy_percent` mantido aberto: com `pread` cada leitura custa ~2 µs
    /// (contra ~30 µs abrindo o arquivo), o que permite amostrar a 50 Hz.
    busy_file: Option<fs::File>,
    busy: BusyAverager,
}

/// Amostrador com estado entre chamadas de [`Sampler::sample`].
pub struct Sampler {
    prev_cpu: Option<(Jiffies, Vec<Jiffies>)>,
    rapl: Option<Rapl>,
    rapl_checked: bool,
    cpu_temp: Option<PathBuf>,
    cpu_temp_checked: bool,
    gpu: Option<GpuPaths>,
    gpu_checked: bool,
    prev_disks: Option<(Instant, Vec<DiskCounters>)>,
    prev_net: Option<(Instant, Vec<NetCounters>)>,
    net_scale: AdaptiveScale,
}

impl Default for Sampler {
    fn default() -> Self {
        Self::new()
    }
}

impl Sampler {
    pub fn new() -> Self {
        Self {
            prev_cpu: None,
            rapl: None,
            rapl_checked: false,
            cpu_temp: None,
            cpu_temp_checked: false,
            gpu: None,
            gpu_checked: false,
            prev_disks: None,
            prev_net: None,
            net_scale: AdaptiveScale::new(NET_SCALE_FLOOR_BPS),
        }
    }

    /// Coleta uma amostra completa. A primeira chamada de CPU/RAPL não tem
    /// delta e devolve uso 0 / watts `None`.
    pub fn sample(&mut self) -> Metrics {
        let mut m = Metrics::default();

        if let Ok(text) = fs::read_to_string("/proc/stat")
            && let Some((agg, cores)) = parse_proc_stat(&text)
        {
            if let Some((prev_agg, prev_cores)) = &self.prev_cpu {
                m.cpu_usage = usage_from_delta(*prev_agg, agg);
                m.cpu_cores = cores
                    .iter()
                    .zip(prev_cores.iter())
                    .map(|(c, p)| usage_from_delta(*p, *c))
                    .collect();
            }
            self.prev_cpu = Some((agg, cores));
        }

        m.cpu_watts = self.sample_rapl();
        m.cpu_temp_c = self.sample_cpu_temp();
        m.cpu_freq_mhz = sample_cpu_freq();
        m.gpu = self.sample_gpu();
        m.mem = fs::read_to_string("/proc/meminfo")
            .ok()
            .and_then(|t| parse_meminfo(&t));
        m.disk = self.sample_disk();
        m.net = self.sample_net();
        m
    }

    /// Atividade dos discos; a primeira chamada só guarda os contadores.
    fn sample_disk(&mut self) -> Option<DiskMetrics> {
        let text = fs::read_to_string("/proc/diskstats").ok()?;
        let cur = parse_diskstats(&text, is_physical_disk);
        let now = Instant::now();
        let prev = self.prev_disks.replace((now, cur));
        let (prev_time, prev) = prev?;
        let (_, cur) = self.prev_disks.as_ref()?;
        let mut d = disk_metrics(&prev, cur, now.duration_since(prev_time).as_secs_f64());
        if let Some((used, total)) = fs_space("/home") {
            d.home_used_bytes = Some(used);
            d.home_total_bytes = Some(total);
        }
        Some(d)
    }

    /// Tráfego de rede; a primeira chamada só guarda os contadores.
    fn sample_net(&mut self) -> Option<NetMetrics> {
        let text = fs::read_to_string("/proc/net/dev").ok()?;
        let cur = parse_net_dev(&text, is_physical_iface);
        let now = Instant::now();
        let (prev_time, prev) = self.prev_net.replace((now, cur))?;
        let (_, cur) = self.prev_net.as_ref()?;
        let dt = now.duration_since(prev_time).as_secs_f64();
        let (rx_bps, tx_bps) = net_rates(&prev, cur, dt);
        let (usage, scale_bps) = self.net_scale.update(rx_bps + tx_bps, dt);
        Some(NetMetrics {
            usage,
            rx_bps,
            tx_bps,
            scale_bps,
        })
    }

    fn discover_rapl(&mut self) {
        self.rapl_checked = true;
        let base = Path::new("/sys/class/powercap");
        let entries = match fs::read_dir(base) {
            Ok(e) => e,
            Err(_) => return, // sem RAPL (CPU AMD antiga, VM, etc.)
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            // Pacote completo: "intel-rapl:0" (sem subdomínios ":0:0").
            if !name.starts_with("intel-rapl:") || name.matches(':').count() != 1 {
                continue;
            }
            let dir = entry.path();
            let energy_path = dir.join("energy_uj");
            let Some(max_range_uj) = read_u64(&dir.join("max_energy_range_uj")) else {
                continue;
            };
            let Some(prev_energy_uj) = read_u64(&energy_path) else {
                continue;
            };
            self.rapl = Some(Rapl {
                energy_path,
                max_range_uj,
                prev_energy_uj,
                prev_time: Instant::now(),
            });
            return;
        }
    }

    fn sample_rapl(&mut self) -> Option<f64> {
        if !self.rapl_checked {
            self.discover_rapl();
        }
        let rapl = self.rapl.as_mut()?;
        let cur = read_u64(&rapl.energy_path)?;
        let now = Instant::now();
        let dt = now.duration_since(rapl.prev_time).as_secs_f64();
        let watts = rapl_watts(rapl.prev_energy_uj, cur, rapl.max_range_uj, dt);
        rapl.prev_energy_uj = cur;
        rapl.prev_time = now;
        watts
    }

    fn discover_cpu_temp(&mut self) {
        self.cpu_temp_checked = true;
        let entries = match fs::read_dir("/sys/class/hwmon") {
            Ok(e) => e,
            Err(_) => return,
        };
        for entry in entries.flatten() {
            let dir = entry.path();
            let Some(name) = read_trim(&dir.join("name")) else {
                continue;
            };
            if matches!(
                name.as_str(),
                "coretemp" | "k10temp" | "zenpower" | "cpu_thermal" | "soc_thermal"
            ) {
                let input = dir.join("temp1_input");
                if input.exists() {
                    self.cpu_temp = Some(input);
                    return;
                }
            }
        }
    }

    fn sample_cpu_temp(&mut self) -> Option<f64> {
        if !self.cpu_temp_checked {
            self.discover_cpu_temp();
        }
        Some(read_f64(self.cpu_temp.as_ref()?)? / 1000.0)
    }

    fn discover_gpu(&mut self) {
        self.gpu_checked = true;
        let entries = match fs::read_dir("/sys/class/drm") {
            Ok(e) => e,
            Err(_) => return,
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            // Só cartões primários ("card0", "card1"...), sem conectores.
            let Some(num) = name.strip_prefix("card") else {
                continue;
            };
            if num.is_empty() || !num.bytes().all(|b| b.is_ascii_digit()) {
                continue;
            }
            let device = entry.path().join("device");
            if !device.join("gpu_busy_percent").exists() {
                continue; // não é amdgpu (ou driver sem métricas)
            }
            let hwmon = fs::read_dir(device.join("hwmon"))
                .ok()
                .and_then(|mut it| it.next())
                .and_then(|e| e.ok())
                .map(|e| e.path());
            let driver = read_trim(&device.join("uevent"))
                .and_then(|text| {
                    text.lines()
                        .find_map(|l| l.strip_prefix("DRIVER=").map(str::to_owned))
                })
                .unwrap_or_else(|| "gpu".to_owned());
            // Falha ao abrir não é fatal: `sample_gpu` cai na leitura direta.
            let busy_file = fs::File::open(device.join("gpu_busy_percent")).ok();
            self.gpu = Some(GpuPaths {
                device,
                hwmon,
                driver,
                busy_file,
                busy: BusyAverager::default(),
            });
            return;
        }
    }

    /// Há GPU com métricas de uso (faz a descoberta na primeira chamada).
    pub fn has_gpu(&mut self) -> bool {
        if !self.gpu_checked {
            self.discover_gpu();
        }
        self.gpu.is_some()
    }

    /// Faz uma leitura rápida do uso da GPU e acumula para a próxima
    /// [`Sampler::sample`]. Feita para ser chamada a ~50 Hz; não faz nada se
    /// não houver GPU com métricas.
    pub fn poll_gpu(&mut self) {
        if !self.gpu_checked {
            self.discover_gpu();
        }
        let Some(paths) = self.gpu.as_mut() else {
            return;
        };
        let Some(file) = &paths.busy_file else {
            return;
        };
        let mut buf = [0u8; 16];
        // Atributos do sysfs regeneram o conteúdo a cada leitura no offset 0.
        // Erro de leitura (ex.: GPU em reset) só perde esta amostra.
        if let Ok(n) = file.read_at(&mut buf, 0)
            && let Some(usage) = parse_busy_percent(&buf[..n])
        {
            paths.busy.push(usage);
        }
    }

    fn sample_gpu(&mut self) -> Option<GpuMetrics> {
        if !self.gpu_checked {
            self.discover_gpu();
        }
        let paths = self.gpu.as_mut()?;
        let dev = &paths.device;
        // Média das leituras rápidas; sem elas (primeira amostra, ou timer
        // rápido desligado), cai numa leitura pontual.
        let usage = match paths.busy.take() {
            Some(avg) => avg,
            None => (read_f64(&dev.join("gpu_busy_percent"))? / 100.0).clamp(0.0, 1.0),
        };
        let mut g = GpuMetrics {
            usage,
            driver: paths.driver.clone(),
            ..GpuMetrics::default()
        };
        g.vram_used_mib = read_u64(&dev.join("mem_info_vram_used")).map(|b| b / 1_048_576);
        g.vram_total_mib = read_u64(&dev.join("mem_info_vram_total")).map(|b| b / 1_048_576);
        if let Some(hw) = &paths.hwmon {
            // hwmon do amdgpu: power1_input em µW, temp1_input em m°C,
            // freq1_input em Hz, fan1_input em RPM.
            g.watts = read_f64(&hw.join("power1_input"))
                .or_else(|| read_f64(&hw.join("power1_average")))
                .map(|uw| uw / 1e6);
            g.temp_c = read_f64(&hw.join("temp1_input")).map(|mc| mc / 1000.0);
            g.freq_mhz = read_f64(&hw.join("freq1_input")).map(|hz| hz / 1e6);
            g.fan_rpm = read_u64(&hw.join("fan1_input")).and_then(|v| u32::try_from(v).ok());
        }
        Some(g)
    }
}

/// Clock médio da CPU em MHz, lido do `/proc/cpuinfo`.
fn sample_cpu_freq() -> Option<f64> {
    parse_cpu_freq(&fs::read_to_string("/proc/cpuinfo").ok()?)
}

/// Média dos campos `cpu MHz` do `/proc/cpuinfo`. O separador é
/// `"cpu MHz\t\t: 2494.730"`, com tabs.
pub fn parse_cpu_freq(text: &str) -> Option<f64> {
    let mut sum = 0.0;
    let mut n = 0u32;
    for line in text.lines() {
        if let Some((key, v)) = line.split_once(':')
            && key.trim() == "cpu MHz"
            && let Ok(v) = v.trim().parse::<f64>()
        {
            sum += v;
            n += 1;
        }
    }
    (n > 0).then_some(sum / f64::from(n))
}

#[cfg(test)]
mod tests {
    use super::*;

    const STAT: &str = "cpu  100 20 30 400 50 10 5 5 0 0\n\
                        cpu0 50 10 15 200 25 5 2 3 0 0\n\
                        cpu1 50 10 15 200 25 5 3 2 0 0\n\
                        intr 123456\n";

    #[test]
    fn parse_agrega_e_nucleos() {
        let (agg, cores) = parse_proc_stat(STAT).expect("stat válido");
        assert_eq!(agg.total, 620);
        assert_eq!(agg.idle, 450);
        assert_eq!(cores.len(), 2);
        assert_eq!(cores[0].total, 310);
    }

    #[test]
    fn parse_sem_cpu_devolve_none() {
        assert!(parse_proc_stat("intr 1\n").is_none());
    }

    #[test]
    fn uso_delta() {
        let prev = Jiffies {
            total: 1000,
            idle: 800,
        };
        let cur = Jiffies {
            total: 1100,
            idle: 850,
        };
        assert!((usage_from_delta(prev, cur) - 0.5).abs() < 1e-9);
        assert_eq!(usage_from_delta(prev, prev), 0.0);
    }

    #[test]
    fn rapl_normal_e_wrap() {
        // 1 J em 1 s = 1 W.
        let w = rapl_watts(1_000_000, 2_000_000, 262_143_328, 1.0);
        assert!((w.expect("delta válido") - 1.0).abs() < 1e-9);
        // Wrap: contador voltou de max-100000 para 500000 → delta 1,5 J.
        let w = rapl_watts(262_143_328 - 1_000_000, 500_000, 262_143_328, 1.0);
        assert!((w.expect("wrap válido") - 1.5).abs() < 1e-9);
        assert!(rapl_watts(0, 1, 100, 0.0).is_none());
    }

    #[test]
    fn cpuinfo_com_tabs() {
        let text = "processor\t: 0\ncpu MHz\t\t: 2494.730\nprocessor\t: 1\ncpu MHz\t\t: 2505.270\n";
        let f = parse_cpu_freq(text).expect("dois núcleos");
        assert!((f - 2500.0).abs() < 1e-9);
        assert!(parse_cpu_freq("processor\t: 0\n").is_none());
    }

    #[test]
    fn meminfo() {
        let text = "MemTotal:       12176768 kB\n\
                    MemFree:         1000000 kB\n\
                    MemAvailable:    8169168 kB\n\
                    SwapTotal:       4194300 kB\n\
                    SwapFree:        4194300 kB\n";
        let m = parse_meminfo(text).expect("meminfo válido");
        assert!((m.usage - (12176768.0 - 8169168.0) / 12176768.0).abs() < 1e-9);
        assert_eq!(m.total_mib, 12176768 / 1024);
        assert_eq!(m.swap_used_mib, 0);
        assert_eq!(m.swap_total_mib, 4194300 / 1024);
        assert!(parse_meminfo("MemTotal: 0 kB\nMemAvailable: 0 kB\n").is_none());
        assert!(parse_meminfo("MemTotal: 100 kB\n").is_none());
    }

    #[test]
    fn diskstats() {
        let text = "   8       0 sda 31652 11062 2957540 14737 6004 15358 4791940 24882 0 18457 42008 0 0 0 0 825 2388\n\
                    8       1 sda1 92 2027 5636 52 2 0 2 0 0 27 52 0 0 0 0 0 0\n\
                    7       0 loop0 1 2 3\n";
        let v = parse_diskstats(text, |n| n == "sda" || n == "loop0");
        assert_eq!(
            v,
            vec![DiskCounters {
                name: "sda".to_owned(),
                sectors_read: 2957540,
                sectors_written: 4791940,
                io_ticks_ms: 18457,
            }],
            "filtra partições e ignora linhas curtas"
        );
    }

    #[test]
    fn disco_util_e_taxa() {
        let c = |name: &str, r, w, t| DiskCounters {
            name: name.to_owned(),
            sectors_read: r,
            sectors_written: w,
            io_ticks_ms: t,
        };
        let prev = [c("sda", 0, 0, 1000), c("sdb", 0, 0, 1000)];
        // Em 2 s: sda ocupado 500 ms (25%), sdb 1500 ms (75%).
        let cur = [
            c("sda", 2048, 0, 1500),
            c("sdb", 0, 4096, 2500),
            c("sdc", 9, 9, 9),
        ];
        let d = disk_metrics(&prev, &cur, 2.0);
        assert!((d.usage - 0.75).abs() < 1e-9);
        assert_eq!(d.busiest, "sdb");
        assert!((d.read_bps - 2048.0 * 512.0 / 2.0).abs() < 1e-6);
        assert!((d.write_bps - 4096.0 * 512.0 / 2.0).abs() < 1e-6);
        // Contador que voltou (disco recolocado) não vira taxa negativa.
        let d = disk_metrics(&cur[..1], &prev[..1], 1.0);
        assert_eq!(d.usage, 0.0);
        assert_eq!(disk_metrics(&prev, &cur, 0.0), DiskMetrics::default());
    }

    #[test]
    fn net_dev() {
        let text = "Inter-|   Receive                                                |  Transmit\n \
             face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed\n    \
             lo: 4158301   10289    0    0    0     0          0         0  4158301   10289    0    0    0     0       0          0\n\
             wlp0s26u1u5: 1669286790 1337586    0   35    0     0          0         0 102083827  542103    0    0    0     0       0          0\n";
        let v = parse_net_dev(text, |n| n != "lo");
        assert_eq!(
            v,
            vec![NetCounters {
                name: "wlp0s26u1u5".to_owned(),
                rx_bytes: 1669286790,
                tx_bytes: 102083827,
            }]
        );
        let prev = [NetCounters {
            name: "wlp0s26u1u5".to_owned(),
            rx_bytes: 1669286790 - 2_000_000,
            tx_bytes: 102083827 - 500_000,
        }];
        let (rx, tx) = net_rates(&prev, &v, 2.0);
        assert!((rx - 1_000_000.0).abs() < 1e-6);
        assert!((tx - 250_000.0).abs() < 1e-6);
    }

    #[test]
    fn escala_adaptativa() {
        let floor = 100.0;
        let mut s = AdaptiveScale::new(floor);
        // Abaixo do piso: fração relativa ao piso.
        assert_eq!(s.update(50.0, 1.0), (0.5, floor));
        // Pico novo: enche na hora.
        assert_eq!(s.update(1000.0, 1.0), (1.0, 1000.0));
        // Depois de uma meia-vida o pico cai a 500, e 250 vira 50%.
        let (frac, scale) = s.update(250.0, AdaptiveScale::HALF_LIFE_SECS);
        assert!((scale - 500.0).abs() < 1e-9);
        assert!((frac - 0.5).abs() < 1e-9);
        // Muito tempo ocioso: volta ao piso, nunca abaixo.
        let (_, scale) = s.update(0.0, 3600.0);
        assert_eq!(scale, floor);
    }

    #[test]
    fn taxa_contador_reiniciado() {
        assert_eq!(rate(100, 50, 1.0), 0.0);
        assert_eq!(rate(0, 100, 0.0), 0.0);
        assert_eq!(rate(0, 100, 2.0), 50.0);
    }

    #[test]
    fn parse_busy() {
        assert_eq!(parse_busy_percent(b"42\n"), Some(0.42));
        assert_eq!(parse_busy_percent(b"0"), Some(0.0));
        assert_eq!(parse_busy_percent(b"250\n"), Some(1.0)); // satura
        assert_eq!(parse_busy_percent(b""), None);
        assert_eq!(parse_busy_percent(b"-1"), None);
        assert_eq!(parse_busy_percent(&[0xff, b'1']), None);
    }

    #[test]
    fn media_de_leituras() {
        let mut avg = BusyAverager::default();
        assert_eq!(avg.take(), None);
        // Padrão real do Polaris sob carga leve: zeros com picos.
        for v in [0.0, 0.0, 0.6, 0.0, 0.0, 0.3, 0.0, 0.0, 0.0, 0.1] {
            avg.push(v);
        }
        let m = avg.take().expect("houve leituras");
        assert!((m - 0.1).abs() < 1e-9);
        assert_eq!(avg.take(), None, "take() zera o acumulador");
        avg.push(2.0); // fora da faixa é limitado
        assert_eq!(avg.take(), Some(1.0));
    }

    /// Simula a onda que o SMU7 entrega (0/100 em rajadas): a média das
    /// leituras a 50 Hz fica perto da ocupação real, enquanto uma leitura
    /// pontual só daria 0 ou 100.
    #[test]
    fn media_converge_para_ocupacao_real() {
        // GPU ocupada 30% do tempo em rajadas de 7 ms a cada 23 ms (~43 Hz).
        let busy_at = |ms: u32| u32::from(ms % 23 < 7);
        let mut avg = BusyAverager::default();
        let mut ms = 3u32;
        while ms < 1000 {
            avg.push(f64::from(busy_at(ms)));
            ms += 20;
        }
        let m = avg.take().expect("houve leituras");
        assert!((m - 7.0 / 23.0).abs() < 0.08, "média {m}");
    }
}
