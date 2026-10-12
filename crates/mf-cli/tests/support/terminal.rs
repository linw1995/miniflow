// Frame diffs can reuse characters from earlier frames, so assertions inspect rendered cells.
pub struct Screen {
    cells: Vec<Vec<char>>,
    row: usize,
    column: usize,
    pending: Vec<u8>,
}

impl Screen {
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            cells: vec![vec![' '; width]; height],
            row: 0,
            column: 0,
            pending: Vec::new(),
        }
    }

    pub fn feed(&mut self, bytes: &[u8]) {
        self.pending.extend_from_slice(bytes);
        let mut index = 0;
        while index < self.pending.len() {
            let byte = self.pending[index];
            if byte == 27 {
                if index + 1 == self.pending.len() {
                    break;
                }
                if self.pending[index + 1] != b'[' {
                    index += 2;
                    continue;
                }
                let Some(end) = (index + 2..self.pending.len())
                    .find(|&i| (0x40..=0x7e).contains(&self.pending[i]))
                else {
                    break;
                };
                if self.pending[end] == b'H' {
                    let parameters = String::from_utf8_lossy(&self.pending[index + 2..end]);
                    let mut coordinates = parameters
                        .split(';')
                        .map(|value| value.parse::<usize>().unwrap_or(1).max(1));
                    self.row = coordinates.next().unwrap_or(1).min(self.cells.len()) - 1;
                    self.column = coordinates.next().unwrap_or(1).min(self.cells[0].len()) - 1;
                }
                index = end + 1;
                continue;
            }
            match byte {
                b'\r' => self.column = 0,
                b'\n' => self.row = (self.row + 1).min(self.cells.len() - 1),
                0..=31 => {}
                _ => {
                    let length = match byte {
                        0..=127 => 1,
                        128..=223 => 2,
                        224..=239 => 3,
                        _ => 4,
                    };
                    if index + length > self.pending.len() {
                        break;
                    }
                    let value = std::str::from_utf8(&self.pending[index..index + length])
                        .unwrap()
                        .chars()
                        .next()
                        .unwrap();
                    if self.column == self.cells[0].len() {
                        self.column = 0;
                        self.row = (self.row + 1).min(self.cells.len() - 1);
                    }
                    self.cells[self.row][self.column] = value;
                    self.column += 1;
                    index += length;
                    continue;
                }
            }
            index += 1;
        }
        self.pending.drain(..index);
    }

    pub fn text(&self) -> String {
        self.cells
            .iter()
            .map(|row| row.iter().collect::<String>().trim_end().to_owned())
            .collect::<Vec<_>>()
            .join("\n")
    }
}
