//! End-to-end checks for `decode_animation`'s three outcomes, using committed
//! APNG fixtures. The 16-bit case is the regression this guards: a real
//! animation the `image` crate can't decode must report `Unsupported` (so the
//! viewer can hint) rather than silently degrading to a static first frame.
#![cfg(feature = "image")]

use tess::image_render::{decode_animation, AnimationDecode};

#[test]
fn eight_bit_apng_decodes_as_animated() {
    let bytes = include_bytes!("fixtures/anim8.apng");
    match decode_animation(bytes) {
        AnimationDecode::Animated(a) => assert_eq!(a.frames.len(), 2),
        AnimationDecode::Static => panic!("8-bit APNG should be Animated, got Static"),
        AnimationDecode::Unsupported(r) => panic!("8-bit APNG should be Animated, got Unsupported: {r}"),
    }
}

#[test]
fn sixteen_bit_apng_is_unsupported_not_silently_static() {
    let bytes = include_bytes!("fixtures/anim16.apng");
    match decode_animation(bytes) {
        AnimationDecode::Unsupported(reason) => {
            assert!(!reason.is_empty(), "Unsupported should carry a reason");
        }
        AnimationDecode::Static => {
            panic!("16-bit APNG must be Unsupported (so the viewer can hint), not silently Static")
        }
        AnimationDecode::Animated(_) => {
            panic!("the image crate cannot decode a 16-bit APNG to frames")
        }
    }
}
