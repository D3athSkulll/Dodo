//! Money as integer cents. No floats in the money path — the assignment checks
//! for this, so `Cents` has no division, no `f64` conversion, no dollar
//! formatting. Only what a billing total needs: add amounts, multiply by an
//! integer quantity.

/// A USD amount in whole cents. Overflowing arithmetic returns `None` so the
/// caller can reject it instead of wrapping or panicking.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Cents(i64);

impl Cents {
    /// Zero — the seed when summing line items.
    pub const ZERO: Cents = Cents(0);

    /// Wrap a raw cent count. No range check here: the schema enforces `>= 0`,
    /// and validation code needs to hold a value it is about to reject.
    pub const fn new(value: i64) -> Self {
        Cents(value)
    }

    /// The raw `i64`, for storage or serialisation.
    pub const fn into_inner(self) -> i64 {
        self.0
    }

    /// Add, returning `None` on overflow. A client can send line items that sum
    /// past `i64::MAX`; that is a validation error, not a wrap.
    pub fn checked_add(self, rhs: Cents) -> Option<Cents> {
        self.0.checked_add(rhs.0).map(Cents)
    }

    /// Line amount: `unit_amount * quantity`. `qty` is a `u32` count, not money,
    /// so it cannot be confused with a `Cents`. `None` on overflow.
    pub fn checked_mul_qty(self, qty: u32) -> Option<Cents> {
        self.0.checked_mul(i64::from(qty)).map(Cents)
    }

    /// Sum, returning `None` if any step overflows. The invoice total goes
    /// through this; `std::iter::Sum` cannot report failure.
    pub fn try_sum<I: IntoIterator<Item = Cents>>(iter: I) -> Option<Cents> {
        iter.into_iter()
            .try_fold(Cents::ZERO, |acc, c| acc.checked_add(c))
    }
}

/// Convenience `sum()` for tests and logging. Saturates to `i64::MAX` on
/// overflow — anything that needs a correct total uses [`Cents::try_sum`].
impl std::iter::Sum for Cents {
    fn sum<I: Iterator<Item = Cents>>(iter: I) -> Cents {
        iter.fold(Cents::ZERO, |acc, c| {
            acc.checked_add(c).unwrap_or(Cents(i64::MAX))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_is_checked() {
        assert_eq!(
            Cents::new(100).checked_add(Cents::new(50)),
            Some(Cents::new(150))
        );
        // Overflow reports rather than wraps.
        assert_eq!(Cents::new(i64::MAX).checked_add(Cents::new(1)), None);
    }

    #[test]
    fn zero_is_the_additive_identity() {
        assert_eq!(
            Cents::ZERO.checked_add(Cents::new(42)),
            Some(Cents::new(42))
        );
    }

    #[test]
    fn mul_qty_widens_then_checks() {
        assert_eq!(Cents::new(250).checked_mul_qty(3), Some(Cents::new(750)));
        // Zero price, huge quantity: still zero.
        assert_eq!(Cents::new(0).checked_mul_qty(u32::MAX), Some(Cents::ZERO));
        // Huge price: overflow reported.
        assert_eq!(Cents::new(i64::MAX).checked_mul_qty(2), None);
    }

    #[test]
    fn try_sum_propagates_overflow() {
        let ok = [Cents::new(1), Cents::new(2), Cents::new(3)];
        assert_eq!(Cents::try_sum(ok), Some(Cents::new(6)));

        let overflow = [Cents::new(i64::MAX), Cents::new(1)];
        assert_eq!(Cents::try_sum(overflow), None);
    }

    #[test]
    fn try_sum_of_nothing_is_zero() {
        assert_eq!(Cents::try_sum(std::iter::empty()), Some(Cents::ZERO));
    }
}
