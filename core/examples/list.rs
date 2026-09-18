//! Prints what `klart` can see and how it reaches each display, which is the
//! only way to check the parts of this that a test on a headless runner cannot.

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let controls = klart_core::controls()?;
    if controls.is_empty() {
        println!("no active displays");
        return Ok(());
    }

    for control in controls {
        let display = control.display();
        let bounds = display.bounds();

        println!(
            "{name}\n  id      {id}\n  key     {key}\n  kind    {kind:?}{main}\n  bounds  {w}x{h} at {x},{y}",
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

        match control.get() {
            Ok(level) => println!(
                "  level   {level} via {mechanism}{caveat}",
                mechanism = control.mechanism().unwrap_or("nothing"),
                caveat = if control.persists() {
                    ""
                } else {
                    " (held only while a process holds it)"
                },
            ),
            Err(failure) => println!("  level   unreadable: {failure}"),
        }

        for refusal in control.refusals() {
            println!("  ...     {refusal}");
        }
        println!();
    }
    Ok(())
}
