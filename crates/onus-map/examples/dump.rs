fn main() {
    let dir = std::env::args().nth(1).unwrap();
    let map = onus_map::build_map(std::path::Path::new(&dir), &Default::default()).unwrap();
    #[allow(clippy::print_stdout)]
    {
        println!("{}", serde_json::to_string_pretty(&map).unwrap());
    }
}
