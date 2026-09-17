//! Prints what `klart` can see, which is the only way to check the parts of
//! display discovery that a test on a headless runner cannot reach.

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let displays = klart_core::displays()?;
    if displays.is_empty() {
        println!("no active displays");
        return Ok(());
    }

    for display in displays {
        let bounds = display.bounds();
        println!(
            "{name}\n  id      {id}\n  key     {key}\n  kind    {kind:?}{main}\n  bounds  {w}x{h} at {x},{y}\n",
            name = display.name(),
            id = display.id(),
            key = display.key(),
            kind = display.kind(),
            main = if display.is_main() { "  (main)" } else { "" },
            w = bounds.width,
            h = bounds.height,
            x = bounds.x,
            y = bounds.y,
        );
    }
    Ok(())
}
