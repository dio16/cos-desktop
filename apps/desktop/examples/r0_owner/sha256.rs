//! Fixed-memory SHA-256 for the development ownership helper.
use std::io::{self, Read};

const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

fn compress(state: &mut [u32; 8], block: &[u8; 64]) {
    let mut w = [0u32; 64];
    for (i, word) in w.iter_mut().take(16).enumerate() {
        let offset = i * 4;
        *word = u32::from_be_bytes([
            block[offset],
            block[offset + 1],
            block[offset + 2],
            block[offset + 3],
        ]);
    }
    for i in 16..64 {
        let a = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
        let b = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
        w[i] = w[i - 16]
            .wrapping_add(a)
            .wrapping_add(w[i - 7])
            .wrapping_add(b);
    }
    let mut a = *state;
    for i in 0..64 {
        let s1 = a[4].rotate_right(6) ^ a[4].rotate_right(11) ^ a[4].rotate_right(25);
        let ch = (a[4] & a[5]) ^ (!a[4] & a[6]);
        let t1 = a[7]
            .wrapping_add(s1)
            .wrapping_add(ch)
            .wrapping_add(K[i])
            .wrapping_add(w[i]);
        let s0 = a[0].rotate_right(2) ^ a[0].rotate_right(13) ^ a[0].rotate_right(22);
        let maj = (a[0] & a[1]) ^ (a[0] & a[2]) ^ (a[1] & a[2]);
        a = [
            t1.wrapping_add(s0.wrapping_add(maj)),
            a[0],
            a[1],
            a[2],
            a[3].wrapping_add(t1),
            a[4],
            a[5],
            a[6],
        ];
    }
    for (word, value) in state.iter_mut().zip(a) {
        *word = word.wrapping_add(value);
    }
}

pub fn hash(mut reader: impl Read) -> io::Result<(String, u64)> {
    let mut state = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let mut block = [0u8; 64];
    let mut used = 0;
    let mut bytes = 0u64;
    loop {
        let count = match reader.read(&mut block[used..]) {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            other => other?,
        };
        if count == 0 {
            break;
        }
        bytes = bytes
            .checked_add(count as u64)
            .ok_or(io::ErrorKind::FileTooLarge)?;
        used += count;
        if used == 64 {
            compress(&mut state, &block);
            used = 0;
        }
    }
    let bits = bytes.checked_mul(8).ok_or(io::ErrorKind::FileTooLarge)?;
    block[used] = 0x80;
    block[used + 1..].fill(0);
    if used >= 56 {
        compress(&mut state, &block);
        block.fill(0);
    }
    block[56..].copy_from_slice(&bits.to_be_bytes());
    compress(&mut state, &block);
    use std::fmt::Write;
    let mut digest = String::with_capacity(71);
    digest.push_str("sha256:");
    for word in state {
        write!(&mut digest, "{word:08x}").map_err(io::Error::other)?;
    }
    Ok((digest, bytes))
}

#[cfg(test)]
mod tests {
    use super::hash;
    use std::io::Read;

    #[test]
    fn independent_vectors_cover_block_and_stream_boundaries() -> std::io::Result<()> {
        let vectors = [
            (
                55usize,
                "9f4390f8d30c2dd92ec9f095b65e2b9ae9b0a925a5258e241c9f1e910f734318",
            ),
            (
                56,
                "b35439a4ac6f0948b6d6f9e3c6af0f5f590ce20f1bde7090ef7970686ec6738a",
            ),
            (
                57,
                "f13b2d724659eb3bf47f2dd6af1accc87b81f09f59f2b75e5c0bed6589dfe8c6",
            ),
            (
                63,
                "7d3e74a05d7db15bce4ad9ec0658ea98e3f06eeecf16b4c6fff2da457ddc2f34",
            ),
            (
                64,
                "ffe054fe7ae0cb6dc65c3af9b61d5209f439851db43d0ba5997337df154668eb",
            ),
            (
                65,
                "635361c48bb9eab14198e76ea8ab7f1a41685d6ad62aa9146d301d4f17eb0ae0",
            ),
            (
                70_000,
                "66915c0872933db504e7578828dd85b7e74a4e0a061f9756793b89c4151bd4b5",
            ),
        ];
        for (length, expected) in vectors {
            let (actual, bytes) = hash(std::io::repeat(b'a').take(length as u64))?;
            assert_eq!(actual, format!("sha256:{expected}"));
            assert_eq!(bytes, length as u64);
        }
        Ok(())
    }
    #[test]
    fn published_sha256_vectors_include_padding_boundaries() -> std::io::Result<()> {
        for (data, expected) in [
            (
                "",
                "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            ),
            (
                "abc",
                "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            ),
            (
                "abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq",
                "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1",
            ),
        ] {
            assert_eq!(
                hash(data.as_bytes())?,
                (format!("sha256:{expected}"), data.len() as u64)
            );
        }
        let million = std::io::repeat(b'a').take(1_000_000);
        assert_eq!(
            hash(million)?,
            (
                "sha256:cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
                    .to_owned(),
                1_000_000
            )
        );
        Ok(())
    }
}
