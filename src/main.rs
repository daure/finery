fn main() -> Result<(), Box<dyn std::error::Error>> {
    tuicore::init();
    finery::run()
}
