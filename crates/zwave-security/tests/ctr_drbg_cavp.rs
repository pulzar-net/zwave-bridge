//! CTR_DRBG against NIST's CAVP vectors: instantiate, reseed, generate twice,
//! compare the second output.

use zwave_security::drbg::CtrDrbg;

fn field<'a>(block: &'a str, name: &str) -> Vec<Vec<u8>> {
    block
        .lines()
        .filter_map(|l| l.split_once(" = ").or_else(|| l.strip_suffix(" =").map(|k| (k, ""))))
        .filter(|(k, _)| k.trim() == name)
        .map(|(_, v)| hex::decode(v.trim()).unwrap())
        .collect()
}

#[test]
fn cavp_aes128_no_df() {
    let data = include_str!("data/ctr_drbg_aes128_nodf.rsp");
    let mut checked = 0;
    for block in data.split("COUNT = ").skip(1) {
        let entropy: [u8; 32] = field(block, "EntropyInput")[0].clone().try_into().unwrap();
        let ps = &field(block, "PersonalizationString")[0];
        let reseed: [u8; 32] = field(block, "EntropyInputReseed")[0].clone().try_into().unwrap();
        let add_reseed = &field(block, "AdditionalInputReseed")[0];
        let adds = field(block, "AdditionalInput");
        let want = &field(block, "ReturnedBits")[0];

        let mut d = CtrDrbg::new(&entropy, ps);
        d.reseed(&reseed, add_reseed);
        let mut out = vec![0u8; want.len()];
        d.generate_with(&mut out, &adds[0]);
        d.generate_with(&mut out, &adds[1]);
        assert_eq!(&out, want, "COUNT {}", block.lines().next().unwrap());
        checked += 1;
    }
    assert_eq!(checked, 60);
}
