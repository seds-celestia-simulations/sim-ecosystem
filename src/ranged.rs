use nutype::nutype;

#[nutype(
    validate(finite),
    derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Display)
)]
pub struct NoNaNF32(f32);

#[nutype(
    validate(finite, greater_or_equal = 0, less_or_equal = 1),
    derive(Clone, Copy, PartialEq, Eq, PartialOrd, Display)
)]
pub struct UnitInterval(f64);

#[nutype(
    validate(finite, greater_or_equal = -1, less_or_equal = 1),
    derive(Clone, Copy, PartialEq, Eq, PartialOrd, Display)
)]
pub struct SignedUnitInterval(f64);

impl UnitInterval {
    pub fn new_clamped(f: f64) -> Self {
        Self::try_new(if f.is_nan() { 0.0 } else { f.clamp(0.0, 1.0) }).unwrap()
    }
}
