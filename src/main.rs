use std::fs::{File, OpenOptions};
use std::os::fd::{AsFd, BorrowedFd};
use std::path::Path;

use anyhow::{Context, Result};
use drm::control::Device as ControlDevice;
use drm::Device as BasicDevice;

struct Card(File);

impl Card {
    fn new(path: impl AsRef<Path>) -> Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .with_context(|| format!("While opening {:?}", path.as_ref()))?;
        Ok(Self(file))
    }
}

// Required to implement drm::Device
impl AsFd for Card {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.0.as_fd()
    }
}

// Required to implement drm::control::Device
impl BasicDevice for Card {}

// Allows modesetting functionality to be performed.
impl ControlDevice for Card {}

fn main() -> Result<()> {
    let card = Card::new("/dev/dri/card0")?;

    let res_handles = card.resource_handles()?;

    for (i, &crtc) in res_handles.crtcs().iter().enumerate() {
        let info = card.get_crtc(crtc)?;

        println!("CRTC {}: {:#?}", i, info);

        let props = card.get_properties(crtc)?;

        let (&ctm, &ctm_val) = props
            .iter()
            .find(|(&phnd, _value)| {
                let info = card.get_property(phnd).unwrap();
                info.name().to_string_lossy() == "CTM"
            })
            .context("Could not find CTM blob")?;

        println!("Current CTM value {}", ctm_val);

        if ctm_val != 0 {
            let ctm_blob = card.get_property_blob(ctm_val)?;
            println!("Current CTM blob {:x?}", ctm_blob);
        }

        let one_s31_32 = 1u64 << 32;

        let matrix = [0, one_s31_32, 0, one_s31_32, 0, 0, 0, 0, one_s31_32 / 2];
        let matrix = card.create_property_blob(&matrix)?;

        card.set_property(crtc, ctm, matrix.into())?;
        card.destroy_property_blob(matrix.into())?;
    }

    Ok(())
}
