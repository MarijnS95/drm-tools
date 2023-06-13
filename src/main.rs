use std::fs::{File, OpenOptions};
use std::os::fd::{AsFd, BorrowedFd};
use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use drm::buffer::{Buffer, DrmFourcc};
use drm::control::atomic::AtomicModeReq;
use drm::control::property::Value;
use drm::control::{AtomicCommitFlags, Device as ControlDevice, PageFlipEvent, PageFlipFlags};
use drm::{Device as BasicDevice, VblankWaitFlags};

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

    card.set_client_capability(drm::ClientCapability::Atomic, true)?;
    card.set_client_capability(drm::ClientCapability::UniversalPlanes, true)?;

    let res_handles = card.resource_handles()?;

    let &con = res_handles.connectors().first().context("No connectors")?;
    let modes = card.get_modes(con).context("Get modes")?;
    let &mode = modes.first().context("No modes")?;
    dbg!(modes);
    // let conn_info = card.get_connector(con, false).context("Get connector")?;
    // dbg!(conn_info);

    for (i, &crtc) in res_handles.crtcs().iter().enumerate() {
        let info = card.get_crtc(crtc)?;

        println!("CRTC {}: {:#?}", i, info);

        let props = card.get_properties(crtc)?;

        let (&active, &active_val) = props
            .iter()
            .find(|(&phnd, _value)| {
                let info = card.get_property(phnd).unwrap();
                info.name().to_string_lossy() == "ACTIVE"
            })
            .context("Could not find ACTIVE blob")?;

        println!("{active:?} details: {:?}", card.get_property(active));
        println!("Current ACTIVE value {}", active_val);

        // for i in 0..5 {
        //     // card.set_property(crtc, active, 0)?;
        //     let mut req = AtomicModeReq::new();
        //     req.add_property(crtc, active, Value::Boolean(i % 2 == 0));
        //     card.atomic_commit(AtomicCommitFlags::ALLOW_MODESET, req)?;

        //     std::thread::sleep(Duration::from_millis(100));

        //     println!("After: {:?}", card.get_property(active));
        // }

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

        let matrix = [one_s31_32, 0, 0, 0, one_s31_32, 0, 0, 0, one_s31_32];
        let matrix = card
            .create_property_blob(&matrix)
            .context("Create property blob")?;

        let (w, h) = mode.size();

        let mut db = card
            .create_dumb_buffer((w as _, h as _), DrmFourcc::Rgba8888, 32)
            .context("Create dumb buffer")?;

        let mut mapping = card.map_dumb_buffer(&mut db)?;
        for (idx, pixel) in mapping.as_mut().iter_mut().enumerate() {
            let col = idx % 4;
            let idx = idx / 4;
            let x = idx % (w as usize) % 600;
            // let y = idx / (w as usize);

            *pixel = match (col, x) {
                (0, 0..=99) => 255,
                (1, 100..=199) => 255,
                (2, 200..=299) => 255,
                (0 | 1, 300..=399) => 255,
                (1 | 2, 400..=499) => 255,
                (0 | 2, 500..=599) => 255,
                _ => 0,
            };
        }

        drop(mapping);

        let fb = card
            .add_framebuffer(&db, 32, 32)
            .context("Add framebuffer")?;

        let mut db2 = card
            .create_dumb_buffer((w as _, h as _), DrmFourcc::Rgba8888, 32)
            .context("Create dumb buffer")?;

        {
            let mut mapping = card.map_dumb_buffer(&mut db2)?;
            mapping.as_mut().fill(255);
        }
        let fb2 = card
            .add_framebuffer(&db2, 32, 32)
            .context("Add framebuffer")?;

        card.set_property(crtc, ctm, matrix.into())
            .context("Set CTM property")?;

        // Note that we're never setting fb2, but we're flipping to it :)
        card.set_crtc(crtc, Some(fb), (0, 0), &[con], Some(mode))
            .context("Set CRTC")?;

        for i in 0..100 {
            let start = Instant::now();

            card.page_flip(
                crtc,
                if i % 2 == 1 { fb2 } else { fb },
                // Generate PageFlip events so that we don't present
                // when a present is in flight (gives EBUSY)
                PageFlipFlags::EVENT,
                None,
                // Some(drm::control::PageFlipTarget::Relative(0)),
            )
            .context("Page flip")?;

            let ev = card.receive_events()?;
            for ev in ev {
                match ev {
                    drm::control::Event::Vblank(_) => println!("Vblank"),
                    drm::control::Event::PageFlip(_) => println!("PageFlip"),
                    drm::control::Event::Unknown(_) => println!("Unknown"),
                }
            }

            // card.wait_vblank(
            //     drm::VblankWaitTarget::Relative(0),
            //     VblankWaitFlags::empty(),
            //     crtc.into(),
            //     0,
            // )
            // .context("Wait vblank")?;

            println!("Flip took {:?}", start.elapsed());
        }

        card.destroy_property_blob(matrix.into())
            .context("Destroy property blob")?;

        card.destroy_framebuffer(fb2)
            .context("Destroy framebuffer")?;

        card.destroy_dumb_buffer(db2)
            .context("Destroy dumb buffer")?;

        card.destroy_framebuffer(fb)
            .context("Destroy framebuffer")?;

        card.destroy_dumb_buffer(db)
            .context("Destroy dumb buffer")?;
    }

    Ok(())
}
