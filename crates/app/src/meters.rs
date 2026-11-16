#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MeterPrefs {
    pub cpu: bool,
    pub ram: bool,
    pub disk: bool,
    pub gpu: bool,
}

pub fn shown(prefs: &MeterPrefs) -> Vec<&'static str> {
    let mut out = Vec::new();
    if prefs.cpu {
        out.push("CPU");
    }
    if prefs.ram {
        out.push("RAM");
    }
    if prefs.disk {
        out.push("Disk");
    }
    if prefs.gpu {
        out.push("GPU");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meters_show_only_what_is_on() {
        assert!(shown(&MeterPrefs::default()).is_empty());
        assert_eq!(
            shown(&MeterPrefs {
                cpu: true,
                disk: true,
                ..MeterPrefs::default()
            }),
            ["CPU", "Disk"]
        );
    }
}
