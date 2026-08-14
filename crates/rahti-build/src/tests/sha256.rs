//! The vectors from FIPS 180-4, plus the edge cases in the padding.

use super::hex;

#[test]
fn the_empty_input_hashes() {
    assert_eq!(
        hex(b""),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
}

#[test]
fn the_one_block_vector_hashes() {
    assert_eq!(
        hex(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}

#[test]
fn the_two_block_vector_hashes() {
    assert_eq!(
        hex(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
        "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
    );
}

/// A million `a`s: the vector that catches a length counter that is too
/// narrow, and the only one that exercises many blocks in a row.
#[test]
fn the_long_vector_hashes() {
    assert_eq!(
        hex(&[b'a'; 1_000_000]),
        "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
    );
}

/// 55 bytes is the largest input that still pads into one block, 56 the
/// smallest that needs a second. Off-by-one here is the classic bug.
#[test]
fn the_padding_boundary_hashes() {
    assert_eq!(
        hex(&[b'a'; 55]),
        "9f4390f8d30c2dd92ec9f095b65e2b9ae9b0a925a5258e241c9f1e910f734318"
    );
    assert_eq!(
        hex(&[b'a'; 56]),
        "b35439a4ac6f0948b6d6f9e3c6af0f5f590ce20f1bde7090ef7970686ec6738a"
    );
}
