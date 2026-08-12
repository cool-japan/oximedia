//! Conformance tests for the parent `inter` module — whole-sequence VP8
//! decode against libvpx-generated references.
//!
//! Split out of `inter.rs` to keep both files well inside this workspace's
//! 2000-line limit; the module is still `dec::inter::tests`, so `use
//! super::*` and the relative `include_bytes!` paths resolve exactly as they
//! would inline.

use super::*;

// --- axis-convention guards (the silent-corruption trap) -----------------

#[test]
fn test_to_mc_maps_row_to_y_and_col_to_x() {
    // A purely *vertical* motion: `mode`'s (row, col) has it in slot 0,
    // `mc`'s (x, y) must have it in slot 1.
    let vertical: super::super::mode::Mv = (-24, 0);
    assert_eq!(to_mc(vertical), (0, -24), "row must land in mc's y slot");

    // A purely *horizontal* motion: the mirror image.
    let horizontal: super::super::mode::Mv = (0, 40);
    assert_eq!(to_mc(horizontal), (40, 0), "col must land in mc's x slot");

    // And an asymmetric one, so a symmetric bug cannot hide.
    assert_eq!(to_mc((3, -7)), (-7, 3));
}

#[test]
fn test_to_mc_all_converts_every_sub_vector() {
    let mut mvs = [(0i16, 0i16); 16];
    for (i, mv) in mvs.iter_mut().enumerate() {
        *mv = (i as i16, -(i as i16));
    }
    let converted = to_mc_all(&mvs);
    for (i, mv) in converted.iter().enumerate() {
        assert_eq!(*mv, (-(i as i16), i as i16), "sub-block {i}");
    }
}

#[test]
fn test_ref_slot_mapping_and_intra_rejection() {
    assert_eq!(ref_slot(RefFrame::Last).expect("last"), RefSlot::Last);
    assert_eq!(ref_slot(RefFrame::Golden).expect("golden"), RefSlot::Golden);
    assert_eq!(ref_slot(RefFrame::AltRef).expect("altref"), RefSlot::AltRef);
    assert!(
        ref_slot(RefFrame::Intra).is_err(),
        "an intra macroblock has no reference surface"
    );
}

#[test]
fn test_narrow_delta_saturates_instead_of_wrapping() {
    assert_eq!(narrow_delta(0), 0);
    assert_eq!(narrow_delta(-63), -63);
    assert_eq!(narrow_delta(63), 63);
    assert_eq!(narrow_delta(1000), 127);
    assert_eq!(narrow_delta(-1000), -128);
}

// --- honesty guards ------------------------------------------------------

#[test]
fn test_inter_frame_before_any_key_frame_is_rejected() {
    let mut dec = Vp8SequenceDecoder::new();
    // Frame tag with bit 0 set == inter frame.
    let err = match dec.decode_frame(&[0x01, 0x00, 0x00]) {
        Err(e) => e,
        Ok(_) => panic!("an inter frame without references must not decode"),
    };
    assert!(matches!(err, CodecError::InvalidBitstream(_)), "{err:?}");
}

#[test]
fn test_empty_payload_is_rejected() {
    let mut dec = Vp8SequenceDecoder::new();
    assert!(dec.decode_frame(&[]).is_err());
}
