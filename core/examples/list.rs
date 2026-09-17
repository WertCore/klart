//! Prints what `klart` can see, which is the only way to check the parts of
//! display discovery and brightness control that a test on a headless runner
//! cannot reach.

use klart_core::{Backend, BuiltIn, Ddc};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let displays = klart_core::displays()?;
    if displays.is_empty() {
        println!("no active displays");
        return Ok(());
    }

    for display in displays {
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

        let mechanism: Option<Box<dyn Backend>> = BuiltIn::open(&display)
            .map(|panel| Box::new(panel) as Box<dyn Backend>)
            .or_else(|panel_refusal| {
                println!("  ...     {panel_refusal}");
                Ddc::open(&display).map(|ddc| Box::new(ddc) as Box<dyn Backend>)
            })
            .ok();

        match mechanism {
            Some(found) => match found.get() {
                Ok(level) => println!("  level   {level} via {}", found.name()),
                Err(failure) => println!("  level   unreadable: {failure}"),
            },
            None => println!("  level   no mechanism reaches this display"),
        }
        println!();
    }
    Ok(())
}
