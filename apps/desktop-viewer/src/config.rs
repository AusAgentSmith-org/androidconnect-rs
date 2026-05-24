use std::io::Read;
use std::time::{SystemTime, UNIX_EPOCH};

use androidconnect_protocol::{DEFAULT_CONTROL_PORT, normalize_pairing_code};
use anyhow::{Result, bail};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub bind: String,
    pub pairing_code: String,
}

pub fn parse_config(args: impl IntoIterator<Item = String>) -> Result<Config> {
    let mut bind = None;
    let mut pairing_code = None;
    let mut args = args.into_iter();

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                bail!("usage: androidconnect-desktop-viewer [BIND] [--pairing-code CODE]");
            }
            "--pairing-code" => {
                let code = args
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--pairing-code requires a value"))?;
                pairing_code = Some(normalize_pairing_code(&code));
            }
            value if value.starts_with("--pairing-code=") => {
                let code = value.trim_start_matches("--pairing-code=");
                pairing_code = Some(normalize_pairing_code(code));
            }
            value if value.starts_with('-') => bail!("unknown argument: {value}"),
            value => {
                if bind.replace(value.to_owned()).is_some() {
                    bail!("multiple bind addresses provided");
                }
            }
        }
    }

    let pairing_code = pairing_code.unwrap_or_else(generate_pairing_code);
    if pairing_code.is_empty() {
        bail!("pairing code cannot be empty");
    }

    Ok(Config {
        bind: bind.unwrap_or_else(|| format!("0.0.0.0:{DEFAULT_CONTROL_PORT}")),
        pairing_code,
    })
}

pub fn generate_pairing_code() -> String {
    let mut bytes = [0_u8; 8];
    if fill_random(&mut bytes).is_err() {
        fill_fallback_random(&mut bytes);
    }
    let value = u64::from_le_bytes(bytes) % 1_000_000;
    format!("{value:06}")
}

fn fill_random(output: &mut [u8]) -> std::io::Result<()> {
    let mut file = std::fs::File::open("/dev/urandom")?;
    file.read_exact(output)
}

fn fill_fallback_random(output: &mut [u8]) {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let mut seed = now ^ ((std::process::id() as u128) << 64);
    for chunk in output.chunks_mut(8) {
        seed ^= seed << 7;
        seed ^= seed >> 9;
        seed = seed.wrapping_mul(0x9e37_79b9_7f4a_7c15);
        let bytes = seed.to_le_bytes();
        chunk.copy_from_slice(&bytes[..chunk.len()]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bind_and_pairing_code() {
        let config = parse_config([
            "127.0.0.1:48172".to_owned(),
            "--pairing-code".to_owned(),
            " 12 ab ".to_owned(),
        ])
        .expect("config");

        assert_eq!(
            config,
            Config {
                bind: "127.0.0.1:48172".to_owned(),
                pairing_code: "12AB".to_owned(),
            }
        );
    }
}
