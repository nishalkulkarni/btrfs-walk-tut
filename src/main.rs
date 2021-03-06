use std::slice;
use std::{
    fs::{File, OpenOptions},
    os::unix::prelude::FileExt,
    path::PathBuf,
};

mod structs;
use structs::*;
mod chunk_tree;
use chunk_tree::{ChunkTreeCache, ChunkTreeKey, ChunkTreeValue};
mod tree;

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

fn bootstrap_chunk_tree(superblock: &BtrfsSuperblock) -> Result<ChunkTreeCache> {
    let array_size = superblock.sys_chunk_array_size as usize;
    let mut offset: usize = 0;
    let mut chunk_tree_cache = ChunkTreeCache::default();

    while offset < array_size {
        let key_size = std::mem::size_of::<BtrfsKey>();
        if offset + key_size > array_size as usize {
            bail!("short key read");
        }

        let key_slice = &superblock.sys_chunk_array[offset..];
        let key = unsafe { &*(key_slice.as_ptr() as *const BtrfsKey) };
        if key.ty != BTRFS_CHUNK_ITEM_KEY {
            bail!(
                "unknown item type={} in sys_array at offset={}",
                key.ty,
                offset
            );
        }

        offset += key_size;

        if offset + std::mem::size_of::<BtrfsChunk>() > array_size {
            bail!("short chunk item read");
        }

        let chunk_slice = &superblock.sys_chunk_array[offset..];
        let chunk = unsafe { &*(chunk_slice.as_ptr() as *const BtrfsChunk) };
        let num_stripes = chunk.num_stripes;
        if num_stripes == 0 {
            bail!("num_stripes cannot be 0");
        }
        if num_stripes != 1 {
            println!(
                "warning: {} stripes detected but only processing 1",
                num_stripes
            );
        }

        let logical = key.offset;
        if chunk_tree_cache.offset(logical).is_none() {
            chunk_tree_cache.insert(
                ChunkTreeKey {
                    start: logical,
                    size: chunk.length,
                },
                ChunkTreeValue {
                    offset: chunk.stripe.offset,
                },
            );
        }

        let chunk_item_size = std::mem::size_of::<BtrfsChunk>()
            + (std::mem::size_of::<BtrfsStripe>() * (chunk.num_stripes as usize - 1));
        if offset + chunk_item_size > array_size {
            bail!("short chunk item + stripe read");
        }
        offset += chunk_item_size;
    }

    Ok(chunk_tree_cache)
}

fn read_chunk_tree_root(
    file: &File,
    chunk_root_logical: u64,
    cache: &ChunkTreeCache,
) -> Result<Vec<u8>> {
    let size = cache
        .mapping_kv(chunk_root_logical)
        .ok_or_else(|| anyhow!("Chunk tree root not bootstrapped"))?
        .0
        .size;
    let physical = cache
        .offset(chunk_root_logical)
        .ok_or_else(|| anyhow!("Chunk tree root not bootstrapped"))?;

    let mut root = vec![0; size as usize];
    file.read_exact_at(&mut root, physical)?;

    Ok(root)
}

fn read_chunk_tree(
    file: &File,
    root: &[u8],
    chunk_tree_cache: &mut ChunkTreeCache,
    superblock: &BtrfsSuperblock,
) -> Result<()> {
    let header = tree::parse_btrfs_header(root).expect("failed to parse chunk root header");

    if header.level == 0 {
        let items = tree::parse_btrfs_leaf(root)?;

        for item in items {
            if item.key.ty != BTRFS_CHUNK_ITEM_KEY {
                continue;
            }

            let chunk = unsafe {
                &*(root
                    .as_ptr()
                    .add(std::mem::size_of::<BtrfsHeader>() + item.offset as usize)
                    as *const BtrfsChunk)
            };

            chunk_tree_cache.insert(
                ChunkTreeKey {
                    start: item.key.offset,
                    size: chunk.length,
                },
                ChunkTreeValue {
                    offset: chunk.stripe.offset,
                },
            );
        }
    } else {
        let ptrs = tree::parse_btrfs_node(root)?;
        for ptr in ptrs {
            let physical = chunk_tree_cache
                .offset(ptr.blockptr)
                .ok_or_else(|| anyhow!("Chunk tree node not mapped"))?;
            let mut node = vec![0; superblock.node_size as usize];
            file.read_exact_at(&mut node, physical)?;
            read_chunk_tree(file, &node, chunk_tree_cache, superblock)?;
        }
    }

    Ok(())
}

fn read_root_tree_root(
    file: &File,
    root_tree_root_logical: u64,
    cache: &ChunkTreeCache,
) -> Result<Vec<u8>> {
    let size = cache
        .mapping_kv(root_tree_root_logical)
        .ok_or_else(|| anyhow!("Root tree root logical addr not mapped"))?
        .0
        .size;

    let physical = cache
        .offset(root_tree_root_logical)
        .ok_or_else(|| anyhow!("Root tree root logical addr not mapped"))?;

    let mut root = vec![0; size as usize];
    file.read_exact_at(&mut root, physical)?;

    Ok(root)
}

fn read_fs_tree_root(
    file: &File,
    superblock: &BtrfsSuperblock,
    root_tree_root: &[u8],
    cache: &ChunkTreeCache,
) -> Result<Vec<u8>> {
    let header =
        tree::parse_btrfs_header(root_tree_root).expect("failed to parse root tree root header");

    if header.level != 0 {
        bail!("Root tree root is not a leaf node");
    }

    let items = tree::parse_btrfs_leaf(root_tree_root)?;
    for item in items.iter().rev() {
        if item.key.objectid != BTRFS_FS_TREE_OBJECTID || item.key.ty != BTRFS_ROOT_ITEM_KEY {
            continue;
        }

        let root_item = unsafe {
            &*(root_tree_root
                .as_ptr()
                .add(std::mem::size_of::<BtrfsHeader>() + item.offset as usize)
                as *const BtrfsRootItem)
        };

        let physical = cache
            .offset(root_item.bytenr)
            .ok_or_else(|| anyhow!("fs tree root not mapped"))?;
        let mut node = vec![0; superblock.node_size as usize];
        file.read_exact_at(&mut node, physical)?;

        return Ok(node);
    }

    bail!("Failed to find root tree item for fs tree root");
}

fn get_inode_ref(
    inode: u64,
    file: &File,
    superblock: &BtrfsSuperblock,
    node: &[u8],
    cache: &ChunkTreeCache,
) -> Result<Option<(BtrfsKey, BtrfsInodeRef, Vec<u8>)>> {
    let header = tree::parse_btrfs_header(node)?;
    // Leaf node
    if header.level == 0 {
        let items = tree::parse_btrfs_leaf(node)?;
        for item in items {
            if item.key.ty != BTRFS_INODE_REF_KEY {
                continue;
            }

            if item.key.objectid == inode {
                let inode_ref = unsafe {
                    &*(node
                        .as_ptr()
                        .add(std::mem::size_of::<BtrfsHeader>() + item.offset as usize)
                        as *const BtrfsInodeRef)
                };

                let inode_ref_payload = unsafe {
                    std::slice::from_raw_parts(
                        (inode_ref as *const BtrfsInodeRef as *const u8)
                            .add(std::mem::size_of::<BtrfsInodeRef>()),
                        inode_ref.name_len.into(),
                    )
                };

                return Ok(Some((item.key, *inode_ref, inode_ref_payload.into())));
            }
        }

        Ok(None)
    } else {
        let ptrs = tree::parse_btrfs_node(node)?;
        for ptr in ptrs {
            let physical = cache
                .offset(ptr.blockptr)
                .ok_or_else(|| anyhow!("fs tree node not mapped"))?;
            let mut node = vec![0; superblock.node_size as usize];
            file.read_exact_at(&mut node, physical)?;
            let ret = get_inode_ref(inode, file, superblock, &node, cache)?;
            if ret.is_some() {
                return Ok(ret);
            }
        }

        Ok(None)
    }
}

fn walk_fs_tree(
    file: &File,
    superblock: &BtrfsSuperblock,
    node: &[u8],
    root_fs_node: &[u8],
    cache: &ChunkTreeCache,
) -> Result<()> {
    let header = tree::parse_btrfs_header(node)?;

    if header.level == 0 {
        let items = tree::parse_btrfs_leaf(node)?;
        for item in items {
            if item.key.ty != BTRFS_DIR_ITEM_KEY {
                continue;
            }

            let dir_item = unsafe {
                &*(node
                    .as_ptr()
                    .add(std::mem::size_of::<BtrfsHeader>() + item.offset as usize)
                    as *const BtrfsDirItem)
            };

            if dir_item.ty != BTRFS_FT_REG_FILE {
                continue;
            }

            let name_slice = unsafe {
                std::slice::from_raw_parts(
                    (dir_item as *const BtrfsDirItem as *const u8)
                        .add(std::mem::size_of::<BtrfsDirItem>()),
                    dir_item.name_len.into(),
                )
            };
            let name = std::str::from_utf8(name_slice)?;

            // Capacity 1 so we don't panic the first `String::insert`
            let mut path_prefix = String::with_capacity(1);
            // `item.key.objectid` is parent inode number
            let mut current_inode_nr = item.key.objectid;

            loop {
                let (current_key, _current_inode, current_inode_payload) =
                    get_inode_ref(current_inode_nr, file, superblock, root_fs_node, cache)?
                        .ok_or_else(|| {
                            anyhow!("Failed to find inode_ref for inode={}", current_inode_nr)
                        })?;
                unsafe { assert_eq!(current_key.objectid, current_inode_nr) };

                if current_key.offset == current_inode_nr {
                    path_prefix.insert(0, '/');
                    break;
                }

                path_prefix.insert_str(
                    0,
                    &format!("{}/", std::str::from_utf8(&current_inode_payload)?),
                );
                current_inode_nr = current_key.offset;
            }
            println!("filename={}{}", path_prefix, name);
        }
    } else {
        let ptrs = tree::parse_btrfs_node(node)?;
        for ptr in ptrs {
            let physical = cache
                .offset(ptr.blockptr)
                .ok_or_else(|| anyhow!("fs tree node not mapped"))?;
            let mut node = vec![0; superblock.node_size as usize];
            file.read_exact_at(&mut node, physical)?;
            walk_fs_tree(file, superblock, &node, root_fs_node, cache)?;
        }
    }

    Ok(())
}

fn main() {
    let opt = Opt::from_args();

    let file = OpenOptions::new()
        .read(true)
        .open(opt.device.as_path())
        .expect("Failed to open path");

    let superblock = parse_superblock(&file).expect("Failed to parse superblock");

    let mut chunk_tree_cache =
        bootstrap_chunk_tree(&superblock).expect("failed to bootstrap chunk tree");

    let chunk_root = read_chunk_tree_root(&file, superblock.chunk_root, &chunk_tree_cache)
        .expect("failed to read chunk tree root");

    read_chunk_tree(&file, &chunk_root, &mut chunk_tree_cache, &superblock)
        .expect("failed to read chunk tree");

    let root_tree_root = read_root_tree_root(&file, superblock.root, &chunk_tree_cache)
        .expect("failed to read root tree root");

    let fs_tree_root = read_fs_tree_root(&file, &superblock, &root_tree_root, &chunk_tree_cache)
        .expect("failed to read fs tree root");

    walk_fs_tree(
        &file,
        &superblock,
        &fs_tree_root,
        &fs_tree_root,
        &chunk_tree_cache,
    )
    .expect("failed to walk fs tree");
}
