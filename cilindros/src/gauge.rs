//! Os medidores disponíveis e a ordem em que aparecem no painel.

use std::ffi::CStr;

use crate::metrics::Metrics;

/// Um cilindro do painel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Cpu,
    Gpu,
    Mem,
    Disk,
    Net,
}

/// Quantidade de medidores (tamanho dos arrays indexados por [`Kind::index`]).
pub const COUNT: usize = 5;

impl Kind {
    /// Todos os medidores, na ordem do painel.
    pub const ALL: [Kind; COUNT] = [Kind::Cpu, Kind::Gpu, Kind::Mem, Kind::Disk, Kind::Net];

    /// Posição em arrays de tamanho [`COUNT`].
    pub fn index(self) -> usize {
        match self {
            Kind::Cpu => 0,
            Kind::Gpu => 1,
            Kind::Mem => 2,
            Kind::Disk => 3,
            Kind::Net => 4,
        }
    }

    /// Rótulo curto desenhado embaixo do cilindro.
    pub fn label(self) -> &'static str {
        match self {
            Kind::Cpu => "CPU",
            Kind::Gpu => "GPU",
            Kind::Mem => "RAM",
            Kind::Disk => "IO",
            Kind::Net => "NET",
        }
    }

    /// Chave na configuração do painel. A da GPU continua `gpu` para manter a
    /// opção de quem já usava o plugin.
    pub fn conf_key(self) -> &'static CStr {
        match self {
            Kind::Cpu => c"cpu",
            Kind::Gpu => c"gpu",
            Kind::Mem => c"mem",
            Kind::Disk => c"disk",
            Kind::Net => c"net",
        }
    }

    /// Texto da opção no diálogo de configuração.
    pub fn conf_label(self) -> &'static CStr {
        match self {
            Kind::Cpu => c"Mostrar CPU",
            Kind::Gpu => c"Mostrar GPU",
            Kind::Mem => c"Mostrar memória",
            Kind::Disk => c"Mostrar atividade de disco",
            Kind::Net => c"Mostrar rede",
        }
    }

    /// Nível atual (0.0..=1.0); medidor sem dados fica vazio.
    pub fn level(self, m: &Metrics) -> f64 {
        match self {
            Kind::Cpu => m.cpu_usage,
            Kind::Gpu => m.gpu.as_ref().map_or(0.0, |g| g.usage),
            Kind::Mem => m.mem.map_or(0.0, |v| v.usage),
            Kind::Disk => m.disk.as_ref().map_or(0.0, |d| d.usage),
            Kind::Net => m.net.map_or(0.0, |n| n.usage),
        }
    }
}

/// Quais medidores aparecem.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Visible([bool; COUNT]);

impl Default for Visible {
    fn default() -> Self {
        Self([true; COUNT])
    }
}

impl Visible {
    pub fn from_flags(flags: [bool; COUNT]) -> Self {
        Self(flags)
    }

    pub fn get(self, k: Kind) -> bool {
        self.0[k.index()]
    }

    /// Medidores visíveis, em ordem. Nunca vazio: com tudo desligado, mostra
    /// a CPU (um plugin sem largura some do painel e não dá para clicar nele).
    pub fn kinds(self) -> Vec<Kind> {
        let v: Vec<Kind> = Kind::ALL.into_iter().filter(|&k| self.get(k)).collect();
        if v.is_empty() { vec![Kind::Cpu] } else { v }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indices_unicos_e_em_ordem() {
        for (i, k) in Kind::ALL.into_iter().enumerate() {
            assert_eq!(k.index(), i);
        }
    }

    #[test]
    fn nunca_vazio() {
        assert_eq!(Visible::from_flags([false; COUNT]).kinds(), vec![Kind::Cpu]);
        let v = Visible::from_flags([false, true, false, false, true]);
        assert_eq!(v.kinds(), vec![Kind::Gpu, Kind::Net]);
        assert_eq!(Visible::default().kinds(), Kind::ALL.to_vec());
    }
}
