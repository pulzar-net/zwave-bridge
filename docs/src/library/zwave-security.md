# zwave-security

Z-Wave security: S0 and S2. [API docs](https://pulzar-net.github.io/zwave-bridge/api/zwave_security/index.html).

| Module | What it has |
|---|---|
| `s0` | S0 encapsulation, nonces and the network key derivation |
| `s2` | S2 Message Encapsulation, SPAN and MPAN nonce state, extensions |
| `kex` | S2 key exchange: X25519 key pairs and the ECDH shared secret |
| `ckdf` | S2's CMAC-based key derivation |
| `drbg` | S2's AES-128 CTR_DRBG, checked against NIST's CAVP vectors |
| `crypto` | AES, AES-CMAC and AES-CCM, from RustCrypto |
| `dsk` | DSKs and PINs: parse, print, compare |
| `qr` | SmartStart QR codes |
| `provisioning` | the SmartStart provisioning list |

`SecurityClass` names the classes (`s2_access_control`,
`s2_authenticated`, `s2_unauthenticated`, `s0_legacy`).
Feature `serde` adds serialization.

Use it on its own for tools that decode captured S2 traffic (given the
keys), validate QR codes, or print DSKs.
