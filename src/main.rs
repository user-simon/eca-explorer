use std::{
    fmt::{self, Display, Formatter}, 
    io, 
    iter, 
    str::FromStr, 
    time::Duration, 
};
use clap::{arg, Parser, ValueEnum};
use crossterm::{
    cursor::{Hide, Show}, 
    event::Event, 
    style::Print, 
    terminal::{EnterAlternateScreen, LeaveAlternateScreen}, 
};
use main_error::MainResult;
use rand::Rng;

/// Rule composed of a boolean outcome for all 8 possible 3-cell neighbourhood combinations. Represented as
/// its Wolfram code. 
#[derive(Clone, Copy)]
struct Rule(u8);

impl Rule {
    /// Applies the rule to a neighbourhood by checking the value of the nth bit, where `n` is the 3-bit
    /// integer contained in `neighbourhood`. 
    fn apply(&self, neighborhood: [bool; 3]) -> bool {
        let [n3, n2, n1] = neighborhood.map(u8::from);
        self.0 & (1 << n1 << (n2 << 1) << (n3 << 2)) != 0
    }
}

/// The sequence of cells getting updated. 
#[derive(Clone, Debug, PartialEq)]
struct Cells(Vec<bool>);

impl Cells {
    fn new_random(width: u16) -> Cells {
        let mut cells = vec![false; width as usize];
        let mut rng = rand::thread_rng();
        rng.fill(&mut cells[..]);
        Cells(cells)
    }

    /// Iterator over all 3-cell neighbourhoods. 
    fn neighborhoods(&self) -> impl Iterator<Item = [bool; 3]> + '_ {
        self.0
            .windows(3)
            .map(TryInto::try_into)
            .map(Result::unwrap)
    }

    // Returns `[first two cells, last two cells]`
    fn edges(&self) -> [[bool; 2]; 2] {
        [self.0.first_chunk::<2>(), self.0.last_chunk::<2>()]
            .map(|x| x.copied())
            .map(|x| x.expect("There are at least 2 cells"))
    }
}

impl FromStr for Cells {
    type Err = &'static str;

    /// Parses a sequence of ones and zeroes as a cell configuration. 
    fn from_str(string: &str) -> Result<Cells, &'static str> {
        if string.len() < 3 {
            return Err("Initial configuration must be at least 3 cells wide")
        }
        string.chars()
            .map(|char| match char {
                '0' => Some(false), 
                '1' => Some(true), 
                _ => None, 
            })
            .collect::<Option<_>>()
            .map(Cells)
            .ok_or("Initial configuration must only contain '0' or '1'")
    }
}

impl Display for Cells {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        let string: String = self.0.iter()
            .map(|cell| match cell {
                false => "╶╴", 
                true => "██", 
            })
            .collect();
        write!(f, "{string}")
    }
}

/// How new values for cells at the very edges should be computed. 
#[derive(ValueEnum, Clone, Copy, Debug)]
enum EdgeHandling {
    /// The previous edge values are retained. 
    Copy, 
    /// Edge neighbours are set to `0`. 
    Crop, 
    /// Edge neighbours wrap around to the other side. 
    Wrap, 
}

/// Run an elementary (one-dimensional) cellular automaton in your terminal. 
#[derive(Parser)]
struct Cli {
    /// The Wolfram code (0-255) of the rule. 
    rule: u8, 

    /// Initial cell configuration. If not specified, a random configuration with the same printed width as
    /// the terminal is used. 
    initial: Option<Cells>, 

    /// How the two edges are handled. 
    #[arg(long, short, default_value="wrap")]
    edges: EdgeHandling, 

    /// Number of generations to run for. If not specified, the terminal height is used. 
    #[arg(long, short)]
    generations: Option<u16>, 

    /// Number of milliseconds to delay before the next generation is computed. 
    #[arg(long, short)]
    delay: Option<u64>, 
}

/// Settings used to run the ECA. 
struct Settings {
    rule: Rule, 
    edge_handling: EdgeHandling, 
    generations: u16, 
    delay: Duration, 
}

/// Computes the next generation from `front` into `back`. Returns `(new front, new back)`. 
fn step(front: Cells, mut back: Cells, settings: &Settings) -> (Cells, Cells) {
    let rule = settings.rule;
    let [left_edge, right_edge] = {
        let [[l1, l2], [r1, r2]] = front.edges();

        match settings.edge_handling {
            EdgeHandling::Copy => [l1, r2], 
            EdgeHandling::Crop => [
                rule.apply([false, l1, l2]), 
                rule.apply([r1, r2, false]), 
            ], 
            EdgeHandling::Wrap => [
                rule.apply([r2, l1, l2]), 
                rule.apply([r1, r2, l1]), 
            ], 
        }
    };
    let [left_edge, right_edge] = [left_edge, right_edge]
        .map(iter::once);
    let middle = front
        .neighborhoods()
        .map(|neighborhood| rule.apply(neighborhood));
    let cells = left_edge
        .chain(middle)
        .chain(right_edge);

    back.0.clear();
    back.0.extend(cells);

    assert_eq!(front.0.len(), back.0.len());

    (back, front)
}

/// Runs all generations of the ECA using double-buffering to minimize allocations (mostly for style points; 
/// the printing of each generation is going to be the bottle-neck, anyways). 
fn run(initial: Cells, settings: Settings) -> io::Result<()> {
    // front allocates the current generation; back allocates the next one
    let mut front = initial;
    let mut back = front.clone();

    for _ in 0..settings.generations {
        // print current generation. explicit `\r` is needed in raw mode
        let string = format!("\n\r{front}");
        crossterm::execute!{
            io::stdout(), 
            Print(string), 
        }?;

        // compute next generation and swap buffers
        (front, back) = step(front, back, &settings);
        
        // end run prematurely if user presses a key (this also delays)
        if crossterm::event::poll(settings.delay)? {
            let _ = crossterm::event::read();
            break
        }
    }

    // wait for user input before exiting
    loop {
        if let Event::Key(_) = crossterm::event::read()? {
            break
        }
    }
    Ok(())
}

// `main_result` is used to pretty-print the error returned from main
fn main() -> MainResult {
    // read arguments from CLI
    let (settings, initial) = {
        let terminal_size = crossterm::terminal::size()?;
        let args = Cli::parse();
        let rule = Rule(args.rule);
        let initial = args.initial.unwrap_or_else(|| {
            let (width, _) = terminal_size;
            Cells::new_random(width / 2) // div by 2 since each cell is 2 chars wide
        });
        let edge_handling = args.edges;
        let generations = args.generations.unwrap_or_else(|| {
            let (_, height) = terminal_size;
            height
        });
        let delay = Duration::from_millis(args.delay.unwrap_or(0));
        let settings = Settings {
            rule, 
            edge_handling, 
            generations, 
            delay, 
        };
        (settings, initial)
    };

    // setup terminal env
    crossterm::terminal::enable_raw_mode()?;
    crossterm::execute!{
        io::stdout(), 
        EnterAlternateScreen, 
        Hide, 
    }?;

    // run all generations and make sure we reset terminal before any error is printed
    let result = run(initial, settings);

    // reset terminal env
    crossterm::execute!{
        io::stdout(), 
        LeaveAlternateScreen, 
        Show, 
    }?;
    crossterm::terminal::disable_raw_mode()?;

    result.map_err(Into::into)
}
