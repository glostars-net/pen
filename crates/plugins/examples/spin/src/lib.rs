wit_bindgen::generate!({
    world: "pen",
    path: "../../wit",
});

struct Spin;

impl Guest for Spin {
    fn init() {
        let mut n = 0u32;
        loop {
            n = n.wrapping_add(1);
            std::hint::black_box(n);
        }
    }
}

export!(Spin);
