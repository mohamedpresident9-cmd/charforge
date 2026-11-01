//! Domain model: what an asset is, how big it is, and how work is split.

use std::fmt;

/// Two quality tiers. The high tier is four times the payload and twice the chunk size.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub enum Quality {
    Low,
    High,
}

impl Quality {
    pub const ALL: [Quality; 2] = [Quality::Low, Quality::High];

    pub fn label(self) -> &'static str {
        match self {
            Quality::Low => "Low",
            Quality::High => "High",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Quality::Low => "low",
            Quality::High => "high",
        }
    }

    /// High quality ships four times the payload of the low tier.
    pub fn multiplier(self) -> u64 {
        match self {
            Quality::Low => 1,
            Quality::High => 4,
        }
    }

    /// Chunk size used when the transfer is split into parts.
    pub fn chunk_size(self) -> u32 {
        match self {
            Quality::Low => 256 * 1024,
            Quality::High => 512 * 1024,
        }
    }
}

/// What the asset represents. Characters are not limited to humans.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum AssetClass {
    Human,
    Animal,
    Car,
    Building,
}

impl AssetClass {
    pub const ALL: [AssetClass; 4] = [
        AssetClass::Human,
        AssetClass::Animal,
        AssetClass::Car,
        AssetClass::Building,
    ];

    pub fn label(self) -> &'static str {
        match self {
            AssetClass::Human => "Humans",
            AssetClass::Animal => "Animals",
            AssetClass::Car => "Cars",
            AssetClass::Building => "Buildings",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            AssetClass::Human => "human",
            AssetClass::Animal => "animal",
            AssetClass::Car => "car",
            AssetClass::Building => "building",
        }
    }
}

/// Payload role of the asset.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum AssetKind {
    /// A rigged mesh + materials.
    Model,
    /// A skeletal animation clip that can be attached to a model.
    Animation,
    /// A clothing piece that can be fitted to a model.
    Clothing,
}

impl AssetKind {
    pub const ALL: [AssetKind; 3] = [AssetKind::Model, AssetKind::Animation, AssetKind::Clothing];

    pub fn label(self) -> &'static str {
        match self {
            AssetKind::Model => "Models",
            AssetKind::Animation => "Animations",
            AssetKind::Clothing => "Clothing",
        }
    }
}

/// Stable identifier. Catalog ids are static strings, so this stays `Copy`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub struct AssetId(pub &'static str);

impl fmt::Display for AssetId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

/// One catalog entry. Sizes are expressed for the low tier and scaled by quality.
#[derive(Clone, Copy, Debug)]
pub struct AssetMeta {
    pub id: AssetId,
    pub name: &'static str,
    pub class: AssetClass,
    pub kind: AssetKind,
    /// Payload size at low quality, in bytes.
    pub base_bytes: u64,
    /// Seed for the deterministic demo source.
    pub seed: u64,
}

impl AssetMeta {
    pub fn bytes(self, quality: Quality) -> u64 {
        self.base_bytes * quality.multiplier()
    }

    pub fn chunk_size(self, quality: Quality) -> u32 {
        quality.chunk_size()
    }

    /// Number of parts the payload is split into. Always covers the whole payload.
    pub fn chunk_count(self, quality: Quality) -> u32 {
        let total = self.bytes(quality);
        let chunk = self.chunk_size(quality) as u64;
        ((total + chunk - 1) / chunk) as u32
    }

    /// Size of one part; the last part may be shorter.
    pub fn chunk_len(self, quality: Quality, index: u32) -> u32 {
        let total = self.bytes(quality);
        let chunk = self.chunk_size(quality) as u64;
        let start = chunk * index as u64;
        if start >= total {
            return 0;
        }
        let remain = total - start;
        remain.min(chunk) as u32
    }
}

/// The static catalog the demo application ships with.
#[derive(Clone)]
pub struct Catalog {
    assets: Vec<AssetMeta>,
}

impl Catalog {
    pub fn load() -> Self {
        let mut assets = Vec::new();

        // Characters are not limited to humans: animals, vehicles and buildings are peers.
        assets.push(asset("human_athlete", "Athlete", AssetClass::Human, AssetKind::Model, 1_310_720, 11));
        assets.push(asset("human_child", "Child", AssetClass::Human, AssetKind::Model, 917_504, 12));
        assets.push(asset("human_elder", "Elder", AssetClass::Human, AssetKind::Model, 1_048_576, 13));
        assets.push(asset("animal_wolf", "Wolf", AssetClass::Animal, AssetKind::Model, 1_179_648, 21));
        assets.push(asset("animal_horse", "Horse", AssetClass::Animal, AssetKind::Model, 1_441_792, 22));
        assets.push(asset("animal_bird", "Bird", AssetClass::Animal, AssetKind::Model, 393_216, 23));
        assets.push(asset("car_sedan", "Sedan", AssetClass::Car, AssetKind::Model, 2_097_152, 31));
        assets.push(asset("car_truck", "Truck", AssetClass::Car, AssetKind::Model, 2_621_440, 32));
        assets.push(asset("building_house", "House", AssetClass::Building, AssetKind::Model, 1_835_008, 41));
        assets.push(asset("building_tower", "Tower", AssetClass::Building, AssetKind::Model, 3_407_872, 42));

        // Animation clips: downloaded to the machine, then applied to a model.
        assets.push(asset("anim_idle", "Idle loop", AssetClass::Human, AssetKind::Animation, 180_224, 51));
        assets.push(asset("anim_walk", "Walk cycle", AssetClass::Human, AssetKind::Animation, 262_144, 52));
        assets.push(asset("anim_run", "Run cycle", AssetClass::Animal, AssetKind::Animation, 311_296, 53));
        assets.push(asset("anim_drive", "Drive by", AssetClass::Car, AssetKind::Animation, 393_216, 54));

        // Clothing: downloaded to the machine, then fitted to a character.
        assets.push(asset("cloth_jacket", "Jacket", AssetClass::Human, AssetKind::Clothing, 229_376, 61));
        assets.push(asset("cloth_boots", "Boots", AssetClass::Human, AssetKind::Clothing, 163_840, 62));
        assets.push(asset("cloth_saddle", "Saddle", AssetClass::Animal, AssetKind::Clothing, 196_608, 63));

        Self { assets }
    }

    pub fn all(&self) -> &[AssetMeta] {
        &self.assets
    }

    pub fn get(&self, id: AssetId) -> Option<AssetMeta> {
        self.assets.iter().copied().find(|a| a.id == id)
    }

    pub fn filter(&self, class: AssetClass, kind: AssetKind) -> Vec<AssetMeta> {
        self.assets
            .iter()
            .copied()
            .filter(|a| a.class == class && a.kind == kind)
            .collect()
    }
}

fn asset(
    id: &'static str,
    name: &'static str,
    class: AssetClass,
    kind: AssetKind,
    base_bytes: u64,
    seed: u64,
) -> AssetMeta {
    AssetMeta {
        id: AssetId(id),
        name,
        class,
        kind,
        base_bytes,
        seed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunks_cover_the_whole_payload_exactly() {
        let catalog = Catalog::load();
        for meta in catalog.all() {
            for quality in Quality::ALL {
                let total: u64 = (0..meta.chunk_count(quality))
                    .map(|i| meta.chunk_len(quality, i) as u64)
                    .sum();
                assert_eq!(total, meta.bytes(quality), "{} {:?}", meta.id, quality);
            }
        }
    }

    #[test]
    fn high_quality_is_four_times_larger() {
        let catalog = Catalog::load();
        for meta in catalog.all() {
            assert_eq!(meta.bytes(Quality::High), meta.bytes(Quality::Low) * 4);
        }
    }

    #[test]
    fn filter_never_mixes_classes() {
        let catalog = Catalog::load();
        for row in catalog.filter(AssetClass::Animal, AssetKind::Model) {
            assert_eq!(row.class, AssetClass::Animal);
            assert_eq!(row.kind, AssetKind::Model);
        }
    }
}
