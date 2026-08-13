//! Exact decoding of PostgreSQL `NUMERIC` from its binary wire format.
//!
//! Every off-the-shelf option loses digits. `f64` has 15–17 significant decimal
//! digits; `rust_decimal` has 28–29 and a fixed 96-bit mantissa. PostgreSQL's
//! `NUMERIC` allows up to 131072 digits before the decimal point and 16383 after,
//! so a client that routes it through either type will silently round real data —
//! in exactly the columns (money, ledger balances) where that matters most.
//!
//! Decoding the wire format directly costs about a hundred lines and is lossless.
//!
//! # Wire format
//!
//! ```text
//! int16  ndigits   number of base-10000 digit groups that follow
//! int16  weight    base-10000 exponent of the first group
//! uint16 sign      0x0000 positive, 0x4000 negative, 0xC000 NaN,
//!                  0xD000 +Infinity, 0xF000 -Infinity
//! uint16 dscale    digits to display after the decimal point
//! int16  digits[ndigits]
//! ```
//!
//! The value is `sum(digits[i] * 10000^(weight - i))`, rendered with exactly
//! `dscale` fractional digits.

const SIGN_POSITIVE: u16 = 0x0000;
const SIGN_NEGATIVE: u16 = 0x4000;
const SIGN_NAN: u16 = 0xC000;
const SIGN_PINF: u16 = 0xD000;
const SIGN_NINF: u16 = 0xF000;

/// Number of decimal digits in one base-10000 group.
const DEC_DIGITS: usize = 4;

/// Decode a `NUMERIC` into its exact decimal representation.
///
/// Returns `None` if the buffer is malformed, so the caller can fall back to
/// showing the value as unsupported rather than panicking on a short read.
pub fn decode(raw: &[u8]) -> Option<String> {
    if raw.len() < 8 {
        return None;
    }
    let ndigits = i16::from_be_bytes([raw[0], raw[1]]);
    let weight = i16::from_be_bytes([raw[2], raw[3]]);
    let sign = u16::from_be_bytes([raw[4], raw[5]]);
    let dscale = u16::from_be_bytes([raw[6], raw[7]]);

    match sign {
        SIGN_NAN => return Some("NaN".to_string()),
        SIGN_PINF => return Some("Infinity".to_string()),
        SIGN_NINF => return Some("-Infinity".to_string()),
        SIGN_POSITIVE | SIGN_NEGATIVE => {}
        _ => return None,
    }

    if ndigits < 0 {
        return None;
    }
    let ndigits = ndigits as usize;
    if raw.len() < 8 + ndigits * 2 {
        return None;
    }

    let digits: Vec<i16> = (0..ndigits)
        .map(|i| {
            let o = 8 + i * 2;
            i16::from_be_bytes([raw[o], raw[o + 1]])
        })
        .collect();
    if digits.iter().any(|d| !(0..10_000).contains(d)) {
        return None;
    }

    let mut out = String::new();
    if sign == SIGN_NEGATIVE {
        out.push('-');
    }

    // Integer part: groups at positions 0..=weight.
    if weight < 0 {
        out.push('0');
    } else {
        for i in 0..=weight as usize {
            let group = digits.get(i).copied().unwrap_or(0);
            if i == 0 {
                // The leading group carries no padding, or "0001" would render
                // as 1 with three spurious leading zeros.
                out.push_str(&group.to_string());
            } else {
                out.push_str(&format!("{group:0DEC_DIGITS$}"));
            }
        }
    }

    // Fractional part: exactly `dscale` digits, taken from the groups after the
    // integer part and padded with zeros where the sender omitted trailing groups.
    let dscale = dscale as usize;
    if dscale > 0 {
        out.push('.');
        let mut frac = String::with_capacity(dscale + DEC_DIGITS);
        // First fractional group index. When weight < -1 the value starts below
        // 10^-4 and the gap must be filled with leading zero groups.
        let first = weight + 1;
        let mut i = first;
        while frac.len() < dscale {
            let group = if i < 0 {
                0
            } else {
                digits.get(i as usize).copied().unwrap_or(0)
            };
            frac.push_str(&format!("{group:0DEC_DIGITS$}"));
            i += 1;
        }
        frac.truncate(dscale);
        out.push_str(&frac);
    }

    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a wire buffer the way the server would.
    fn wire(weight: i16, sign: u16, dscale: u16, digits: &[i16]) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(&(digits.len() as i16).to_be_bytes());
        v.extend_from_slice(&weight.to_be_bytes());
        v.extend_from_slice(&sign.to_be_bytes());
        v.extend_from_slice(&dscale.to_be_bytes());
        for d in digits {
            v.extend_from_slice(&d.to_be_bytes());
        }
        v
    }

    #[test]
    fn decodes_a_simple_fraction() {
        // 1234.5678
        assert_eq!(
            decode(&wire(0, SIGN_POSITIVE, 4, &[1234, 5678])).as_deref(),
            Some("1234.5678")
        );
    }

    #[test]
    fn decodes_zero() {
        assert_eq!(
            decode(&wire(0, SIGN_POSITIVE, 0, &[])).as_deref(),
            Some("0")
        );
        assert_eq!(
            decode(&wire(0, SIGN_POSITIVE, 2, &[])).as_deref(),
            Some("0.00")
        );
    }

    #[test]
    fn decodes_negatives() {
        assert_eq!(
            decode(&wire(0, SIGN_NEGATIVE, 2, &[42, 5000])).as_deref(),
            Some("-42.50")
        );
    }

    #[test]
    fn leading_group_is_not_zero_padded() {
        // weight 1, digits [1, 0] is 10000, not "00010000".
        assert_eq!(
            decode(&wire(1, SIGN_POSITIVE, 0, &[1, 0])).as_deref(),
            Some("10000")
        );
    }

    #[test]
    fn interior_groups_are_zero_padded() {
        // 1_0001 → groups [1, 1] at weight 1 → "1" then "0001".
        assert_eq!(
            decode(&wire(1, SIGN_POSITIVE, 0, &[1, 1])).as_deref(),
            Some("10001")
        );
    }

    #[test]
    fn values_below_one_get_a_leading_zero() {
        // 0.5 → weight -1, one group of 5000, dscale 1.
        assert_eq!(
            decode(&wire(-1, SIGN_POSITIVE, 1, &[5000])).as_deref(),
            Some("0.5")
        );
    }

    #[test]
    fn small_magnitudes_pad_with_leading_zero_groups() {
        // 0.00005 → the first significant group sits at weight -2, so a whole
        // group of zeros must be emitted before it. Off-by-one here is the
        // classic bug in hand-rolled numeric decoders.
        assert_eq!(
            decode(&wire(-2, SIGN_POSITIVE, 5, &[5000])).as_deref(),
            Some("0.00005")
        );
    }

    #[test]
    fn trailing_groups_are_padded_to_the_display_scale() {
        // dscale asks for 8 fractional digits but only one group was sent.
        assert_eq!(
            decode(&wire(0, SIGN_POSITIVE, 8, &[1, 5000])).as_deref(),
            Some("1.50000000")
        );
    }

    #[test]
    fn precision_beyond_f64_survives_intact() {
        // 38 significant digits — far past f64 (≈17) and rust_decimal (≈29).
        // This is the case the whole module exists for.
        let digits = [1234, 5678, 9012, 3456, 7890, 1234, 5678, 9012, 3456, 7890];
        let s = decode(&wire(4, SIGN_POSITIVE, 20, &digits)).expect("decode");
        assert_eq!(
            s,
            "1234567890123456789012345678901234567890"[..20].to_string()
                + "."
                + &"1234567890123456789012345678901234567890"[20..40]
        );
        assert_eq!(s, "12345678901234567890.12345678901234567890");
    }

    #[test]
    fn special_values_are_named_not_numeric() {
        assert_eq!(decode(&wire(0, SIGN_NAN, 0, &[])).as_deref(), Some("NaN"));
        assert_eq!(
            decode(&wire(0, SIGN_PINF, 0, &[])).as_deref(),
            Some("Infinity")
        );
        assert_eq!(
            decode(&wire(0, SIGN_NINF, 0, &[])).as_deref(),
            Some("-Infinity")
        );
    }

    #[test]
    fn malformed_buffers_return_none_instead_of_panicking() {
        // A truncated or corrupt buffer must degrade to "unsupported", never
        // take down the query with an index panic.
        assert_eq!(decode(&[]), None);
        assert_eq!(decode(&[0, 1, 0, 0]), None);
        // Claims two digit groups but supplies one.
        let mut short = wire(0, SIGN_POSITIVE, 0, &[1]);
        short[0..2].copy_from_slice(&2i16.to_be_bytes());
        assert_eq!(decode(&short), None);
        // Digit group out of the base-10000 range.
        assert_eq!(decode(&wire(0, SIGN_POSITIVE, 0, &[10_000])), None);
        // Unrecognized sign.
        assert_eq!(decode(&wire(0, 0x1234, 0, &[1])), None);
    }

    #[test]
    fn round_trips_the_scale_exactly() {
        // Money columns are NUMERIC(_, 2): trailing zeros are significant and
        // must not be trimmed.
        assert_eq!(
            decode(&wire(0, SIGN_POSITIVE, 2, &[100])).as_deref(),
            Some("100.00")
        );
    }
}
