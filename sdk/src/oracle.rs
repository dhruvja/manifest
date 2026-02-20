use anyhow::{anyhow, Result};
use solana_client::rpc_client::RpcClient;
use solana_program::pubkey::Pubkey;

/// Fetch a Pyth V2 push oracle price.
///
/// Returns `(mantissa, exponent, price_usd)` where the order price is
/// `mantissa * 10^exponent` in quote_atoms/base_atom, and `price_usd` is the
/// human-readable USD price.
pub fn fetch_pyth_v2_price(
    client: &RpcClient,
    feed: &Pubkey,
    quote_decimals: u8,
    base_decimals: u8,
) -> Result<(u32, i8, f64)> {
    const PYTH_MAGIC: u32 = 0xa1b2c3d4;
    const EXPO_OFF: usize = 20;
    const PRICE_OFF: usize = 208;
    const STATUS_OFF: usize = 224;
    const STATUS_TRADING: u32 = 1;

    let data = client.get_account_data(feed)?;
    if data.len() < 240 {
        return Err(anyhow!(
            "Pyth account too small ({} bytes). Is this really a Pyth V2 price account?",
            data.len()
        ));
    }
    let magic = u32::from_le_bytes(data[0..4].try_into().unwrap());
    if magic != PYTH_MAGIC {
        return Err(anyhow!(
            "Pyth magic mismatch: got {:#010x}, expected {:#010x}",
            magic,
            PYTH_MAGIC
        ));
    }
    let expo = i32::from_le_bytes(data[EXPO_OFF..EXPO_OFF + 4].try_into().unwrap());
    let price = i64::from_le_bytes(data[PRICE_OFF..PRICE_OFF + 8].try_into().unwrap());
    let status = u32::from_le_bytes(data[STATUS_OFF..STATUS_OFF + 4].try_into().unwrap());

    if status != STATUS_TRADING {
        return Err(anyhow!("Pyth price not in Trading status: {status}"));
    }
    if price <= 0 {
        return Err(anyhow!("Pyth price non-positive: {price}"));
    }

    let price_usd = price as f64 * 10f64.powi(expo);

    let combined_expo = expo + quote_decimals as i32 - base_decimals as i32;
    let mut mantissa = price;
    let mut order_expo = combined_expo;

    while mantissa > u32::MAX as i64 {
        mantissa /= 10;
        order_expo += 1;
    }
    while mantissa > 0 && mantissa % 10 == 0 {
        mantissa /= 10;
        order_expo += 1;
    }
    if order_expo < i8::MIN as i32 || order_expo > i8::MAX as i32 {
        return Err(anyhow!("Order exponent {order_expo} out of i8 range"));
    }

    Ok((mantissa as u32, order_expo as i8, price_usd))
}

/// Parse a Pyth `PriceUpdateV3` account (used on MagicBlock ER).
///
/// Returns the human-readable USD price.
pub fn parse_price_v3(data: &[u8]) -> Result<f64> {
    if data.len() < 93 {
        return Err(anyhow!(
            "PriceUpdateV3 account too small ({} bytes)",
            data.len()
        ));
    }
    let msg_start: usize = match data[40] {
        0x01 => 41,
        0x00 => 42,
        b => return Err(anyhow!("Unknown VerificationLevel byte: {:#04x}", b)),
    };
    if data.len() < msg_start + 52 {
        return Err(anyhow!("PriceUpdateV3 truncated at message payload"));
    }
    let price = i64::from_le_bytes(data[msg_start + 32..msg_start + 40].try_into().unwrap());
    let expo = i32::from_le_bytes(data[msg_start + 48..msg_start + 52].try_into().unwrap());
    if price <= 0 {
        return Err(anyhow!("PriceUpdateV3 price non-positive: {price}"));
    }
    Ok(price as f64 / 10f64.powi(expo))
}

/// Fetch a price from the ER oracle (PriceUpdateV3 format).
///
/// Returns `(mantissa, exponent, price_usd)`.
pub fn fetch_er_price(
    client: &RpcClient,
    feed: &Pubkey,
    quote_decimals: u8,
    base_decimals: u8,
) -> Result<(u32, i8, f64)> {
    let data = client.get_account_data(feed)?;
    let price_usd = parse_price_v3(&data)?;
    let (m, e) = usd_to_order_price(price_usd, quote_decimals, base_decimals);
    Ok((m, e, price_usd))
}

/// Fetch price trying V2 first, falling back to V3.
///
/// Returns `(mantissa, exponent, price_usd)`.
pub fn fetch_price(
    client: &RpcClient,
    feed: &Pubkey,
    quote_decimals: u8,
    base_decimals: u8,
) -> Result<(u32, i8, f64)> {
    fetch_pyth_v2_price(client, feed, quote_decimals, base_decimals).or_else(|_| {
        fetch_er_price(client, feed, quote_decimals, base_decimals)
    })
}

/// Convert a human USD price to order `(mantissa, exponent)`.
///
/// The resulting order price is `mantissa * 10^exponent` in quote_atoms/base_atom.
pub fn usd_to_order_price(price_usd: f64, quote_decimals: u8, base_decimals: u8) -> (u32, i8) {
    let mut m = price_usd;
    let mut e: i32 = quote_decimals as i32 - base_decimals as i32;
    while (m - m.floor()).abs() > 1e-9 && m < u32::MAX as f64 / 10.0 {
        m *= 10.0;
        e -= 1;
    }
    while m > u32::MAX as f64 {
        m /= 10.0;
        e += 1;
    }
    (m.round() as u32, e as i8)
}
