//! Code-defined HRIR catalog: maps known HeSuVi profile stems to display name,
//! vendor group, tonality, license class, and shipping origin. Drives the picker
//! and gates which assets the app may bundle/redistribute (permissive only).

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tonality {
    Dry,
    Roomy,
    Neutral,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum License {
    Permissive,
    Proprietary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    Bundled,
    Import,
    Fetch,
}

#[derive(Debug, Clone, Copy)]
pub struct HrirCatalogEntry {
    pub stem: &'static str,
    pub display: &'static str,
    pub group: &'static str,
    pub tonality: Tonality,
    pub license: License,
    pub origin: Origin,
}

const CATALOG: &[HrirCatalogEntry] = &[
    HrirCatalogEntry {
        stem: "07-oal+++-openal-max",
        display: "OpenAL (Max)",
        group: "OpenAL",
        tonality: Tonality::Dry,
        license: License::Permissive,
        origin: Origin::Bundled,
    },
    HrirCatalogEntry {
        stem: "04-gsx-sennheiser-gsx",
        display: "Sennheiser GSX",
        group: "Sennheiser",
        tonality: Tonality::Dry,
        license: License::Proprietary,
        origin: Origin::Import,
    },
    HrirCatalogEntry {
        stem: "06-cmss-game-creative-cmss3d",
        display: "Creative CMSS-3D",
        group: "Creative",
        tonality: Tonality::Dry,
        license: License::Proprietary,
        origin: Origin::Import,
    },
    HrirCatalogEntry {
        stem: "05-sbx67-sbx-pro-studio",
        display: "Creative SBX Pro Studio",
        group: "Creative",
        tonality: Tonality::Neutral,
        license: License::Proprietary,
        origin: Origin::Import,
    },
    HrirCatalogEntry {
        stem: "02-dh-dolby-headphone",
        display: "Dolby Headphone",
        group: "Dolby",
        tonality: Tonality::Roomy,
        license: License::Proprietary,
        origin: Origin::Import,
    },
    HrirCatalogEntry {
        stem: "03-dht-dolby-atmos-headphones",
        display: "Dolby Atmos",
        group: "Dolby",
        tonality: Tonality::Roomy,
        license: License::Proprietary,
        origin: Origin::Import,
    },
    HrirCatalogEntry {
        stem: "08-dtshx-dts-headphone-x",
        display: "DTS Headphone:X",
        group: "DTS",
        tonality: Tonality::Roomy,
        license: License::Proprietary,
        origin: Origin::Import,
    },
    HrirCatalogEntry {
        stem: "10-waves-nx",
        display: "Waves NX",
        group: "Waves",
        tonality: Tonality::Roomy,
        license: License::Proprietary,
        origin: Origin::Import,
    },
    HrirCatalogEntry {
        stem: "12-ssc-ny-sonic-studio-ny",
        display: "Sonic Studio NY",
        group: "Spatial Sound Card",
        tonality: Tonality::Roomy,
        license: License::Proprietary,
        origin: Origin::Import,
    },
];

pub fn catalog() -> &'static [HrirCatalogEntry] {
    CATALOG
}

pub fn entry_for(stem: &str) -> Option<&'static HrirCatalogEntry> {
    CATALOG.iter().find(|e| e.stem == stem)
}

pub fn display_name(stem: &str) -> String {
    if let Some(e) = entry_for(stem) {
        return e.display.to_string();
    }
    // Heuristic fallback: strip leading "NN-", split on - and _, title-case.
    let no_prefix = stem
        .split_once('-')
        .and_then(|(h, t)| {
            if h.chars().all(|c| c.is_ascii_digit()) && !h.is_empty() {
                Some(t)
            } else {
                None
            }
        })
        .unwrap_or(stem);
    no_prefix
        .split(['-', '_'])
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                Some(f) => f.to_uppercase().chain(c).collect::<String>(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_stem_resolves_display_group_and_license() {
        let e = entry_for("04-gsx-sennheiser-gsx").expect("gsx in catalog");
        assert_eq!(e.display, "Sennheiser GSX");
        assert_eq!(e.group, "Sennheiser");
        assert!(matches!(e.license, License::Proprietary));
    }

    #[test]
    fn permissive_openal_is_marked_permissive_and_bundled() {
        let e = entry_for("07-oal+++-openal-max").expect("openal in catalog");
        assert!(matches!(e.license, License::Permissive));
        assert!(matches!(e.origin, Origin::Bundled));
    }

    #[test]
    fn unknown_stem_falls_back_to_humanized_name() {
        assert_eq!(display_name("12-foo-bar-baz"), "Foo Bar Baz");
    }
}
