/*
    This file is a part of telegram-media-deserialize.

    Copyright (C) 2022 Apple Sheeple <AppleSheeple at github>

    telegram-media-deserialize is free software: you can
    redistribute it and/or modify it under the terms of
    the Affero GNU General Public License as published by
    the Free Software Foundation.

    This program is distributed in the hope that it will be useful,
    but WITHOUT ANY WARRANTY; without even the implied warranty of
    MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the
    Affero GNU General Public License for more details.

    You should have received a copy of the Affero GNU General Public License
    along with this program.  If not, see <http://www.gnu.org/licenses/>.
*/

/// Telegram Desktop's cached `media_cache` can be decrypted using a python script available here:
/// https://github.com/lilydjwg/telegram-cache-decryption
///
/// You may notice than not all decrypted media files are playable, and there are no files
/// that are larger than 10MiB.
///
/// Telegram Desktop (as of Dec 2022) seem to split larger media files into multiple cache
/// files, the first of which is serialized for streaming purposes. Other cache files may
/// not exist if the media is not fully cached.
///
/// Serialization is simple, the serialized cache file contains one or more *slices*, each
/// slice is split into multiple *parts*.
///
/// A *slice* header is simply 4 bytes indicating the number of parts in it.
///
/// A *part* header is simply 8 bytes, with the first four indicating the deserialized media
/// stream offset, followed by four bytes indicating the part byte size.
///
/// Note that parts are not necessarily contiguous, or ordered over multiple slices. The reader
/// side of this serialized cache file emulates a media player, so if an MP4 file has a moov atom
/// necessary for playback at the end of the media file, the reader will seek to the end and read
/// from there, then come back (in the next slice).
/// 
/// The next split cache files are not serialized, and can simply be appended. **But** it should be
/// noted that parts written with a forward seek (as described above) leaving a hole in
/// the deserialized stream should be discarded. In-order data written to the deserialized file
/// wouldn't exceed 8MiB (Check 'Last contiguous offset' value in program output).
///
/// Final note, there are a few bytes left after the parsed slices in the serialized file. I don't
/// know what they are. But simply discarding them worked for me.
/// 

use std::env;
use std::path::PathBuf;
use std::fs::{File, Metadata, OpenOptions};
use std::io::{Read, Write, Seek, SeekFrom};

type Res<T> = Result<T, String>;

#[derive(Debug)]
struct DeserializedFile {
    name: String,
    file: File,
}

impl DeserializedFile {
    fn from_name(name: String) -> Res<Self> {
        let path  = PathBuf::from(name.clone());

        (!path.exists())
            .then_some(())
            .ok_or_else(|| format!("'{name}' already exists"))?;


        let file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(path)
            .map_err(|e| format!("failed to create '{name}' for writing: {e}"))?;

        Ok(Self {name, file})
    }

    fn _seek_from_start(&mut self, offset: u64) -> Res<u64> {
        self.file.seek(SeekFrom::Start(offset))
            .map_err(|e| format!("failed to seek '{}' at offset={offset}: {e}", self.name))
    }
}

#[derive(Debug)]
struct PartInfo {
    in_offset: u64,
    out_offset: u32,
    part_size: u32,
}

struct OrderedPartInfos(Vec<PartInfo>);


#[derive(Debug)]
struct SerializedFile {
    name: String,
    metadata: Metadata,
    file: File,
    rd_buf: [u8; 4096],
    b4_buf: [u8; 4],
}

impl SerializedFile {
    fn from_name(name: String) -> Res<Self> {
        let path  = PathBuf::from(name.clone());
        path.exists()
            .then_some(())
            .ok_or_else(|| format!("'{name}' not accessible or does not exist"))?;

        let file = OpenOptions::new()
            .read(true)
            .open(path)
            .map_err(|e| format!("failed to open '{name}' for read: {e}"))?;

        let metadata = file.metadata()
            .map_err(|e| format!("failed to get metadata for '{name}': {e}"))?;

        let rd_buf = [0; 4096];
        let b4_buf = [0; 4];

        Ok(Self {name, metadata, file, rd_buf, b4_buf})
    }

    fn _seek_from_start(&mut self, offset: u64) -> Res<u64> {
        self.file.seek(SeekFrom::Start(offset))
            .map_err(|e| format!("failed to seek '{}' to offset={offset}: {e}", self.name))
    }

    fn _seek_from_curr(&mut self, offset: i64) -> Res<u64> {
        self.file.seek(SeekFrom::Current(offset))
            .map_err(|e| format!("failed to seek '{}' from current position with offset={offset}: {e}", self.name))
    }

    fn _get_pos(&mut self) -> Res<u64> {
        self.file.stream_position()
            .map_err(|e| format!("getting stream position of '{}' failed: {e}", self.name))
    }

    fn _read_u32_le(&mut self) -> Res<u32> {
        self.file.read_exact(&mut self.b4_buf)
            .map_err(|e| format!("reading 4 bytes from '{}' failed: {e}", self.name))?;

        Ok(u32::from_le_bytes(self.b4_buf))
    }

    fn read_part(&mut self, part_size: u32) -> Res<Vec<u8>> {
        let part_size = usize::try_from(part_size)
            .map_err(|_| format!("failed to convert {part_size}u64 to a usize value"))?;
        let mut part_buf = Vec::with_capacity(part_size);
        'rd: loop {
            match self.file.read(&mut self.rd_buf) {
                Ok(n) => {
                    let n2 = n.min(part_size - part_buf.len());
                    part_buf.extend_from_slice(&self.rd_buf[0..n2]);
                    //eprintln!("read {n} bytes, save {n2} bytes, part_buf len={}", part_buf.len());
                    if part_buf.len() == part_size {
                        break 'rd;
                    }
                },
                Err(e) => {
                    let total_read = part_buf.len();
                    (total_read == part_size)
                        .then_some(())
                        .ok_or_else(|| format!("failed to read part of size {part_size} from {}, \
                                only {total_read} bytes read: {e}", self.name))?;
                    break 'rd;
                }
            }
        }
        assert_eq!(part_buf.len(), part_size);
        Ok(part_buf)
    }

    fn order_and_report_info(mut info: Vec<PartInfo>) -> OrderedPartInfos {
        info.sort_by_key(|pi| pi.out_offset);

        match info.len() {
            0 | 1 => (),
            len => { 
                let mut last_contigous_i = 0;
                for i in 1..len {
                    let prev = &info[i-1];
                    let curr = &info[i];
                    if curr.out_offset == prev.out_offset + prev.part_size {
                        last_contigous_i = i;
                    }
                }
                // report
                let first_part = &info[0];
                let last_part = &info[len-1];
                let last_contiguous = &info[last_contigous_i];
                let last_contiguous_offset = last_contiguous.out_offset + last_contiguous.part_size;
                let discontinuity_len = last_part.out_offset - last_contiguous_offset;
                eprintln!("\n=======\nAfter ordering part info by out_offset:\n \
                            First part: {first_part:?}\n \
                            Last contiguous: {last_contiguous:?}\n \
                            Last contiguous offset: {last_contiguous_offset} (Discontinuity: {discontinuity_len} bytes)\n \
                            Last part: {last_part:?}\n=======");
            },
        }

        OrderedPartInfos(info)
    }

    fn get_info(&mut self) -> Res<OrderedPartInfos> {
        const MAX_PARTS_COUNT: u32 = 80;
        const MAX_PART_SIZE: u32 = 128 * 1024;

        let mut ret_vec = Vec::with_capacity(128);

        let _ = self._seek_from_start(0)?;

        let mut slice_i = 0;
        let mut in_offset = 0;
        // TODO: loop limit in-case a bad file is encountered
        'out: while in_offset < self.metadata.len() {
            let parts_res = self._read_u32_le();

            if parts_res.is_err() {
                eprintln!("reached EOF, will stop parsing..");
                break 'out;
            }

            let parts = parts_res?;

            if parts == 0 || parts > MAX_PARTS_COUNT {
                eprintln!("Slice{slice_i}: in_offset={in_offset}, \
                    parsed parts={parts} is zero or > max allowed({MAX_PARTS_COUNT}), will stop parsing..");
                eprintln!("in_offset={in_offset}, stopped parsing with {} bytes remaining in file.", self.metadata.len() - in_offset);
                break 'out;
            }
            eprintln!("Slice{slice_i}: in_offset={in_offset}, parts={parts}");

            let mut read_parts = 0;

            while read_parts < parts {
                in_offset = self._get_pos()?;

                let out_offset = self._read_u32_le()?;
                let part_size = self._read_u32_le()?;

                if part_size == 0 || part_size > MAX_PART_SIZE {
                    eprintln!("Slice{slice_i}/Part{read_parts}: in_offset={in_offset}, \
                        part_size={part_size} is zero or > max_allowed({MAX_PART_SIZE}), will stop parsing..");
                    eprintln!("in_offset={in_offset}, stopped parsing with {} bytes remaining in file.", self.metadata.len() - in_offset);
                    break 'out;
                }

                in_offset = self._get_pos()?;
                eprintln!("Slice{slice_i}/Part{read_parts}: in_offset={in_offset}, out_offset={out_offset}, part_size={part_size}");
                ret_vec.push(PartInfo{in_offset, out_offset, part_size});

                in_offset = self._seek_from_curr(part_size as i64)?;
                read_parts += 1;
            }
            slice_i += 1;
        }
        Ok(Self::order_and_report_info(ret_vec))
    }

    fn write_to_deserialized_file(&mut self, mut deserialized_file: DeserializedFile) -> Res<()> {
            let ordered_info = self.get_info()?;
        for PartInfo{in_offset, out_offset, part_size} in ordered_info.0 {
            let _ = self._seek_from_start(in_offset)?;
            let part_bytes = self.read_part(part_size)?;
            let _ = deserialized_file._seek_from_start(out_offset.into())?;
            eprintln!("writing {part_size} from {}@{in_offset} to {}@{out_offset}", self.name, deserialized_file.name);
            deserialized_file.file.write_all(&part_bytes)
                .map_err(|e| format!("failed to write part(size={part_size}) to {}@{out_offset}: {e}", self.name))?;
        }
        Ok(())
    }
}

fn main() -> Res<()> {
    const USAGE: &str = "Usage: telegram-media-deserialize <serialized_file> <deserialized_file>";
    let mut args = env::args();

    let _exec = args.next().expect(USAGE);
    let serialized_file = args.next().expect(USAGE);
    let deserialized_file = args.next().expect(USAGE);

    args.next().is_none().then_some(()).expect(USAGE);

    let mut serialized_file = SerializedFile::from_name(serialized_file)?;
    let deserialized_file = DeserializedFile::from_name(deserialized_file)?;

    serialized_file.write_to_deserialized_file(deserialized_file)
}
