wit_bindgen::generate!({
    world: "pen",
    path: "../../wit",
});

struct Hello;

impl Guest for Hello {
    fn init() {
        let _ = pen::host::host::register_command("Hello");
    }
}

export!(Hello);
