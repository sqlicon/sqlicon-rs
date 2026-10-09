//! Base-100 radix complement arithmetic for Informix wire format.

/// Performs an in-place base-100 radix complement over digits in range 0..99.
/// Used for 10's-complement encoding of negative decimals, datetimes, and intervals.
pub fn radix_complement(groups: &mut [u8]) {
    const RADIX: u32 = 100;
    let mut carry: u32 = 1;
    for slot in groups.iter_mut().rev() {
        let digit = RADIX - 1 - (*slot as u32) + carry;
        *slot = (digit % RADIX) as u8;
        carry = digit / RADIX;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_radix_complement_identity() {
        let original = vec![1, 23, 45, 0];
        let mut complement = original.clone();
        radix_complement(&mut complement);
        // Radix complement twice returns to original
        radix_complement(&mut complement);
        assert_eq!(original, complement);
    }
}
