use unclip_measure::MeasurementProfile;

fn requires_order<T: Ord>() {}

fn main() {
    requires_order::<MeasurementProfile>();
}
