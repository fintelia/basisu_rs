# basisu_rs

I'm writing a [Basis Universal](https://github.com/BinomialLLC/basis_universal) decoder in Rust to learn more about compression and optimization through [learning in public](https://www.mentalnodes.com/the-only-way-to-learn-in-public-is-to-build-in-public).

The code is WIP and not fit for any practical usage.

Specification: [.basis File Format and ETC1S Texture Video Specification](https://github.com/BinomialLLC/basis_universal/wiki/.basis-File-Format-and-ETC1S-Texture-Video-Specification)

Sample textures were copied from the official [basis_universal repo](https://github.com/BinomialLLC/basis_universal/tree/d0ee14e1fb34ce92adf877a20e3a8226ced6dcdd/webgl/texture/assets) under Apache License 2.0.


## Progress

- [x] Reading FileHeader
- [x] CRC-16
- [x] Reading SliceDesc
- [x] LSB-first bit reader
- [x] Decoding Huffman tables
- [x] Decoding endpoint codebooks
- [x] Decoding selector codebooks
- [x] Decoding ETC1S slices
- [x] Textures with flipped Y
- [x] Textures with dimensions not divisible by 4
- [x] Writing out ETC1 textures
- [x] Lookup tables for faster Huffman decoding
- [ ] Decoding UASTC

## Log

Here I'm writing a log of what I did, problems I encountered, and what I learned. Have anything to say or discuss? I'd be happy to hear from you, please send me a DM or @ me on Twitter [@JakubValtar](https://twitter.com/jakubvaltar).

### 27-06-2020

I decided to optimize Huffman decoding today, to get the low-hanging fruit. I still run a linear search to find the right entry in the decoding table.

First I switched to a sparse table ([6824f2](https://github.com/JakubValtar/basisu_rs/commit/6824f262293c0435e53db7c8c32cd1ca86dcbe4a)). I added the symbol to the table entry, which allowed me to get rid of empty entries and to sort the table by code size, since the entries are not tied to the position in the table anymore. This led to a 2x speedup, because shorter codes appear more frequently in the bit stream.

Next I switched to a lookup table ([46ffc8](https://github.com/JakubValtar/basisu_rs/commit/46ffc8752fe78639c542719a2dbd7e8d4f5e1f47)). I decided to keep it simple and start with a full table. The maximum code size is 16 bits, which means 2^16 entries. Each entry is copied into all the slots which end with the code of the entry. This led to a 15x speedup.

The last thing I did today was to make the size of the lookup table adapt to the size of the longest code present ([29e323](https://github.com/JakubValtar/basisu_rs/commit/29e3233c3e55741211dc9cdf41f8f7ba36843fc4)). The decoding table is constructed from a slice of code sizes of the symbols, so it's easy to find the longest code. Instead of 2^16, the lookup table size is now 2^max_code_size. This helps, because half of the tables have at most 21 entries and we don't have to waste time on generating lookup tables with 2^16 entries. This led to a 1.5x speedup.

The combined speedup for today ended up being around 45x. I'm pretty happy with this :)

### 26-06-2020

Implemented writing out ETC1 textures, I used the [Khronos spec](https://www.khronos.org/registry/DataFormat/specs/1.1/dataformat.1.1.html#ETC1). I refactored selectors to use less space and added a field which stores the values in ETC1 format. Since selectors are reused, preparing this data beforehand makes much more sense than recalculating it for every block.

I found a bug in the RGBA decoding in the process. The ETC1 spec says that 5-bit color values should be converted to 8-bit by shifting three bits left and replicating top three bits into the bottom three bits. I wasn't replicating the bits, so there was some loss of quality, although barely noticeable.

There doesn't seem to be any easy way to export the ETC1 data in some file format which could be opened by some engine or texture viewer. I will have to look into validating the ETC1 output later.

### 22-06-2020

Now handling textures with dimensions not divisible by 4 and textures with Y flipped.

### 20-06-2020

Just a little quality of life improvement today: added a byte reader to simplify reading structs.

### 07-06-2020

I implemented ETC1S slice decoding, it was mostly a rewrite of the code in the spec (again). I checked the [unity blog - Crunch compression of ETC textures](https://blogs.unity3d.com/2017/12/15/crunch-compression-of-etc-textures/) to learn more about ETC1 and how to decode endpoints and selectors into RGBA data. I added PNG export to check that the textures are being decoded correctly.

I was getting garbage images at first, because I was reading from a wrong part of the file. During debugging, I went through the selector decoding code again and simplified it to a bare minimum to reduce complexity. It's probably somewhat slower now, but it's clear what's going on and there will be time for optimization later.

Last but not least, I organized the crate a bit to make space for upcoming tasks: writing out ETC1S textures, which can be decoded by the graphics card, and decoding UASTC textures.

### 01-06-2020

I added functions to read endpoints and selectors. This was basically a rewrite of the code from the spec, I need to go through it tomorrow again to get a better grasp of what is happening and how the selectors are stored.

I'm not sure how to test this code, I think I will have to wait till I have the slice decoding working, then I can verify CRC-16 of the decoded texture data for each ETC1S slice.

### 31-05-2020

I added a test for the bit reader and fixed a bug which I found.

I implemented a simple Huffman decoding function which decodes a bitstream into symbols. It does a linear search through the codes in the decoding table until if finds a match. Decoding could be optimized by storing the symbol in the table entry and sorting the entries by frequency (code size). Ideally there should be a lookup table, but I'm not adding it right now, because I want to have a working decoder first and make optimizations later.

The decoding didn't work at first, because the codes in the Huffman tables (as generated with the code from the Deflate RFC 1951) expect a bit encounter order `MSB -> LSB`, but the bit reader returns the bits in order `LSB -> MSB`. Reversing the bit order of the codes in the decoding table solved this problem.

Now that the Huffman decoding works, I'm able to decode all the Huffman tables in the file.

Another issue was that if the highest 16-bit code was used, the `next_code` would overflow and crash the program. Making `next_code` 32-bit fixed this and also allowed me to add a check for codes going higher than `u16::MAX`. I could have kept it in 16 bits and used `wrapping_add` instead, but then I wouldn't be able to check if the overflowed codes were used or not.

### 30-05-2020

I wrote my own LSB-first bit reader, it's naive and will need to be optimized later. The code length array is now looking reasonable and I can convert it into a Huffman table by using the algorithm written in the Deflate spec.

### 29-05-2020

I had only half an hour today, but managed to figure it out. Turns out you can read bits in two ways, MSB-first or LSB-first. To get some intuition for this, I highly recommend [Reading bits in far too many ways (part 1)](https://fgiesen.wordpress.com/2018/02/19/reading-bits-in-far-too-many-ways-part-1/) by Fabian Giesen. I blindly used a create which silently assumed MSB-first. Silly me.

What made the whole situation worse is that the data is tightly packed into only as many bits as needed to represent all valid values. If you read something from a wrong offset or if you even reverse the bits, you still get reasonably looking value. I have to be careful about this when dealing with compression. I can't rely on getting obviously wrong values when I make a mistake, I need to validate the data in more sophisticated ways.

### 28-05-2020

Wrapping my head around Huffman tables (Section 6.0 of the spec). I don't have experience with Huffman coding or Deflate, so I kind of expected this would be the difficult part. I got a pretty good understanding of Huffman coding after reading the [Huffman coding section of Modern LZ Compression](https://glinscott.github.io/lz/index.html#toc3) and the [section 3.2. of Deflate - RFC 1951](https://tools.ietf.org/html/rfc1951) (linked from the Basis spec).

Sadly my understanding did not translate well into practice. I got already stuck on reading the first array of code lengths. The bit stream contains a variable number of code lengths, plus they are sent in a different order than the order used for creating the Huffman table. This confused me a lot and I had to take a look at the [transcoder implementation](https://github.com/BinomialLLC/basis_universal/blob/6ef114ac1e0665b233c04fcb2e1249400ec65044/contrib/previewers/lib/basisu_transcoder.h#L919) to figure out what all of this means. Including the code to read this array in the spec would help a lot.

I finally managed to read the first code length array, but the lengths did not represent a valid Huffman table. This created more confusion. Am I reading at the right offset? Are these really code lenghts, or do I need to decode these somehow to get valid code lengths?

### 27-05-2020

Reading the file header and slice descriptions was pretty straighforward. The only surprise was the use of 24-bit integers, Rust does not have 24-bit primitive types, luckily `ByteOrder` crate I used can read them into `u32`. I could have used `packed_struct` or something similar instead of reading all the fields manually, but for now I don't think it's worth the complexity.

Implementing CRC-16 was super easy, the code provided in the spec is short, clear, and easy to convert. Worked on the first try.
