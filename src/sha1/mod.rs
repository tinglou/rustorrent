use sha1::Digest;

#[cfg(target_arch = "x86_64")]
mod x86_sha;

pub fn sha1(data: &[u8]) -> [u8; 20] {
    #[cfg(any(target_arch = "x86_64"))]
    {
        if is_x86_feature_detected!("sha")
            && is_x86_feature_detected!("sse2")
            && is_x86_feature_detected!("ssse3")
            && is_x86_feature_detected!("sse4.1")
        {
            return unsafe { x86_sha::compute_sha1(data) };
        }
    }

    // TODO: Implement for ARM

    // Fallback: use the extern crate sha1
    let mut hasher = sha1::Sha1::new();
    hasher.update(data);
    let result = hasher.finalize();
    result.into()
}

#[cfg(test)]
mod tests {
    use sha1::Digest;

    use super::sha1;

    #[test]
    fn extern_vs_us_16k() {
        use rand::RngCore;

        let mut rng = rand::thread_rng();

        let mut vec = vec![0; 16 * 1024];

        rng.fill_bytes(&mut vec);

        let res1 = sha1(&vec);

        let mut m = sha1::Sha1::new();
        m.update(&vec);
        let res2: [u8; 20] = m.finalize().into();

        assert_eq!(res1, res2);
    }

    // #[test]
    // fn empty() {
    //     let res1 = sha1(&[]);
    // }
}
