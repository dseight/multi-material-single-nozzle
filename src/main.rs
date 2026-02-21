use std::env;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, BufWriter, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

struct SlicerConfig {
    wipe_tower: bool,
    total_toolchanges: u32,
}

impl SlicerConfig {
    pub fn from_file(file: &File) -> io::Result<SlicerConfig> {
        let reader = BufReader::new(file);
        Self::read(reader)
    }

    pub fn read(reader: impl BufRead) -> io::Result<SlicerConfig> {
        let mut config = SlicerConfig {
            wipe_tower: false,
            total_toolchanges: 0,
        };

        for line in reader.lines() {
            let line = line?;
            config.update_from_line(&line);
        }

        Ok(config)
    }

    fn update_from_line(&mut self, line: &str) {
        if let Some(val) = line.strip_prefix("; total toolchanges = ") {
            if let Ok(n) = val.parse() {
                self.total_toolchanges = n;
                return;
            }
        }

        if let Some(val) = line.strip_prefix("; wipe_tower = ") {
            self.wipe_tower = match val {
                "1" => true,
                "0" => false,
                _ => self.wipe_tower,
            };
            return;
        }
    }
}

fn replace_unloads(
    reader: impl BufRead,
    writer: &mut impl Write,
    total_toolchanges: u32,
) -> io::Result<()> {
    let mut skip_block = false;
    let mut toolchanges = 0;

    for line in reader.lines() {
        let line = line?;

        // "CP TOOLCHANGE UNLOAD ... CP TOOLCHANGE WIPE" is nested inside
        // of "CP TOOLCHANGE START ... CP TOOLCHANGE END", thus checked first
        if line.starts_with("; CP TOOLCHANGE UNLOAD") {
            skip_block = true;
            continue;
        }
        if line.starts_with("; CP TOOLCHANGE WIPE") {
            writeln!(writer, "M600")?;
            skip_block = false;
            continue;
        }

        if line.starts_with("; CP TOOLCHANGE START") {
            toolchanges += 1;

            // The last "CP TOOLCHANGE START ... CP TOOLCHANGE END" block
            // must be removed completely
            if toolchanges > total_toolchanges {
                skip_block = true;
                continue;
            }
        }
        if line.starts_with("; CP TOOLCHANGE END") {
            if toolchanges > total_toolchanges {
                skip_block = false;
                continue;
            }
        }

        if !skip_block {
            writer.write_all(line.as_bytes())?;
            writer.write(b"\n")?;
        }
    }

    writer.flush()
}

fn replace_toolchanges(reader: impl BufRead, writer: &mut impl Write) -> io::Result<()> {
    let mut skip_block = false;

    for line in reader.lines() {
        let line = line?;

        if line.starts_with("; CP TOOLCHANGE START") {
            skip_block = true;
            continue;
        }
        if line.starts_with("; CP TOOLCHANGE END") {
            writeln!(writer, "M600")?;
            skip_block = false;
            continue;
        }

        if !skip_block {
            writer.write_all(line.as_bytes())?;
            writer.write(b"\n")?;
        }
    }

    writer.flush()
}

fn tempfile(prefix: &str) -> io::Result<(File, PathBuf)> {
    let mut path = env::temp_dir();

    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    path.push(format!("{}_{}", prefix, ts));

    let file = File::create(&path)?;

    Ok((file, path))
}

fn main() -> io::Result<()> {
    let args: Vec<String> = env::args().collect();

    if args.len() != 2 || args.iter().any(|arg| arg == "-h" || arg == "--help") {
        println!("Usage: multi-material-single-nozzle <file>");
        println!("Cleans up PrusaSlicer G-code to use single nozzle multi-material setup.");
        println!("Version: {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    let input_path = &args[1];
    let mut input_file = File::open(input_path)?;

    let config = SlicerConfig::from_file(&input_file)?;
    input_file.seek(SeekFrom::Start(0))?;

    let reader = BufReader::new(input_file);

    let (temp_file, temp_path) = tempfile("gcode")?;
    let mut writer = BufWriter::new(temp_file);

    if config.wipe_tower {
        replace_unloads(reader, &mut writer, config.total_toolchanges)?;
    } else {
        replace_toolchanges(reader, &mut writer)?;
    }

    fs::rename(&temp_path, input_path)?;

    println!("Success: '{}' processed.", input_path);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_replace_toolchanges() {
        let input = "\
G1 X149.27 Y134.713 E.46499
M204 P2500
;--------------------
; CP TOOLCHANGE START
; toolchange #1
; material : PETG -> PETG
;--------------------
M220 B
M220 S100
; CP TOOLCHANGE UNLOAD
;WIDTH:1
;WIDTH:0.5
G4 S0
M486 S-1
; ...
G1 X103.329
; CP TOOLCHANGE WIPE
; ...
G92 E0
; CP TOOLCHANGE END
;------------------

G1 X102.279 Y135.586 F7200
; ...
;--------------------
; CP TOOLCHANGE START
; toolchange #2
; material : PETG -> PETG
;--------------------
M220 B
M220 S100
; CP TOOLCHANGE UNLOAD
;WIDTH:1
;WIDTH:0.5
G4 S0
M486 S-1
; ...
G1 X106.954
; CP TOOLCHANGE WIPE
; ...
G92 E0
; CP TOOLCHANGE END
;------------------
";
        let expected = "\
G1 X149.27 Y134.713 E.46499
M204 P2500
;--------------------
M600
;------------------

G1 X102.279 Y135.586 F7200
; ...
;--------------------
M600
;------------------
";

        let mut output = Vec::new();
        replace_toolchanges(input.as_bytes(), &mut output).unwrap();
        let result = String::from_utf8(output).unwrap();
        assert_eq!(result, expected);
    }

    #[test]
    fn test_replace_unloads() {
        let input = "\
M204 P2500
;--------------------
; CP TOOLCHANGE START
; toolchange #1
;--------------------
M220 S100
; CP TOOLCHANGE UNLOAD
G4 S0
; ...
G1 X103.329
; CP TOOLCHANGE WIPE
G92 E0
; CP TOOLCHANGE END
;------------------
G1 X198.749 Y158.4 E.03097
M204 P2500
M486 S-1
;HEIGHT:0.15
;TYPE:Wipe tower
;WIDTH:0.5
;--------------------
; CP TOOLCHANGE START
M220 S100
; CP TOOLCHANGE UNLOAD
G4 S0
M220 R
G1 X102.529 Y135.836 F18000
G4 S0
G92 E0
; CP TOOLCHANGE END
;------------------
G1 E-.8 F2100
";
        let expected = "\
M204 P2500
;--------------------
; CP TOOLCHANGE START
; toolchange #1
;--------------------
M220 S100
M600
G92 E0
; CP TOOLCHANGE END
;------------------
G1 X198.749 Y158.4 E.03097
M204 P2500
M486 S-1
;HEIGHT:0.15
;TYPE:Wipe tower
;WIDTH:0.5
;--------------------
;------------------
G1 E-.8 F2100
";

        let mut output = Vec::new();
        replace_unloads(input.as_bytes(), &mut output, 1).unwrap();
        let result = String::from_utf8(output).unwrap();
        assert_eq!(result, expected);
    }

    #[test]
    fn test_no_toolchanges() {
        let input = "G1 X10\nG1 Y10\n";
        let mut output = Vec::new();
        replace_toolchanges(input.as_bytes(), &mut output).unwrap();
        assert_eq!(String::from_utf8(output).unwrap(), input);
    }

    #[test]
    fn test_config_read() {
        let config_data = "\
; CP TOOLCHANGE START
; toolchange #1
; ...
; CP TOOLCHANGE START
; toolchange #2
; ...
; CP TOOLCHANGE START
; toolchange #3
; ...
; CP TOOLCHANGE START
; toolchange #4
; ...
; total toolchanges = 4
; estimated first layer printing time (normal mode) = 6m 47s
; estimated first layer printing time (silent mode) = 6m 51s

; prusaslicer_config = begin
; arc_fitting = emit_center
; ...
; wipe_tower = 1
; prusaslicer_config = end
";

        let config = SlicerConfig::read(config_data.as_bytes()).unwrap();
        assert_eq!(config.wipe_tower, true);
        assert_eq!(config.total_toolchanges, 4);
    }
}
