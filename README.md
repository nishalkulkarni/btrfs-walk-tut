## btrfs-walk-tut

Prints the absolute path of all regular files in an unmounted btrfs filesystem image.

Learning about btrfs: [Btrfs Basics Series](https://dxuuu.xyz/btrfs-internals.html)

This repo is almost 1:1 copy of: 
[Danobi Original Repo](https://github.com/danobi/btrfs-walk) 

### Setup
```
# Create image file
truncate -s 1G image

mkfs.btrfs image

sudo mkdir /mnt/btrfs

sudo mount image /mnt/btrfs

# Create a few files directories inside
sudo touch a.txt
sudo touch b.txt
sudo mkdir test
sudo touch test/c.txt
sudo touch test/d.txt

sudo umount /mnt/btrfs 
```

### Usage
```
cargo run <path_to_image>
```
#### OR
```
cargo build
./target/debug/btrfs-walk-tut <path_to_image>
```

#### Sample Output
```
warning: 2 stripes detected but only processing 1
filename=/a.txt
filename=/b.txt
filename=/nishal/c.txt
filename=/nishal/d.txt
```