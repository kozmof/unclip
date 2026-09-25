use std::marker::PhantomData;
use unclip_epistemic::{ops, Derived};

fn unavailable<T>() -> T {
    panic!("compile-only fixture")
}

fn main() {
    let _ = Derived::<u32, ops::Calculation> {
        id: unavailable(),
        value: 1,
        provenance: unavailable(),
        operation: PhantomData,
    };
}
