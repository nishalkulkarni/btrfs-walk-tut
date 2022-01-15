use std::slice;
use std::{
    fs::{File, OpenOptions},
    os::unix::prelude::FileExt,
    path::PathBuf,
};

mod structs;
use structs::BtrfsSuperblock;

use anyhow::{anyhow, bail, Result};
use structopt::StructOpt;

const BTRFS_SUPERBLOCK_OFFSET: u64 = 0x10_000;
const BTRFS_SUPERBLOCK_MAGIC: [u8; 8] = *b"_BHRfS_M";

#[derive(Debug, StructOpt)]
#[structopt(
    name = "btrfs-tut",
    about = "Prints the absolute path of all regular files in an unmounted btrfs filesystem image"
)]
struct Opt {
    /// Block device or file to process
    #[structopt(parse(from_os_str))]
    device: PathBuf,
}

fn parse_superblock(file: &File) -> Result<BtrfsSuperblock> {
    let mut superblock: BtrfsSuperblock = unsafe { std::mem::zeroed() };
    let superblock_size = std::mem::size_of::<BtrfsSuperblock>();

    let slice;
    unsafe {
        slice = slice::from_raw_parts_mut(&mut superblock as *mut _ as *mut u8, superblock_size);
    }
    file.read_exact_at(slice, BTRFS_SUPERBLOCK_OFFSET)?;

    if superblock.magic != BTRFS_SUPERBLOCK_MAGIC {
        bail!("superblock magic is wrong");
    }

    Ok(superblock)
}

fn main() {
    println!("Hello, world!");
    let opt = Opt::from_args();

    println!("{:?}", opt.device.as_path());
    let file = OpenOptions::new()
        .read(true)
        .open(opt.device.as_path())
        .expect("Failed to open path");

    let superblock = parse_superblock(&file).expect("Failed to parse superblock");

    println!("{:?}", superblock.total_bytes);
}
