# Build

```
cargo build --release
```

Executable file should exist at `target/release/telegram-media-deserialize`.

That's it. 

# Usage

```
telegram-media-deserialize <serialized_file> <deserialized_file>
```

Make note of 'Last contiguous offset' info printed (see below).
------

# Info

Telegram Desktop's media (Videos/Audios) are cached in `media_cache`
(usually at: `~/.local/share/TelegramDesktop/tdata/user_data/media_cache`)
and can be decrypted using a python script available here:

https://github.com/lilydjwg/telegram-cache-decryption

You may notice that not all decrypted media files are playable, and there are no files
that are larger than 10MiB.

Telegram Desktop (as of Dec 2022) seem to split larger media files into multiple cache
files, the first of which is serialized for streaming purposes (other split cache files
may not exist if the media is not fully cached).

Serialization is simple, the serialized cache file contains one or more *slices*, each
slice is split into multiple *parts*.

A *slice* header is simply 4 bytes indicating the number of parts in it.

A *part* header is simply 8 bytes, with the first four indicating the deserialized media
stream offset, followed by four bytes indicating the part byte size.

Note that parts are not necessarily contiguous, or ordered over multiple slices. The reader
side of this serialized cache file emulates a media player, so if an MP4 file has a moov atom
necessary for playback at the end of the media file, the reader will seek to the end and read
from there, then come back (in the next slice).

The next split cache files are not serialized, and can simply be appended. **But** it should be
noted that parts written with a forward seek (as described above) leaving a hole in
the deserialized stream should be discarded. Check the ***Last contiguous offset*** value in
program output.

Final note, there are a few bytes left after the parsed slices in the serialized file. I don't
know what they are. But simply discarding them worked for me.
