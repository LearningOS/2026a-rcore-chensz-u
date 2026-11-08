use super::{
    block_cache_sync_all, get_block_cache, BlockDevice, DirEntry, DiskInode, DiskInodeType,
    EasyFileSystem, BLOCK_SZ, DIRENT_SZ, INDIRECT2_BOUND, NAME_LENGTH_LIMIT,
};
use crate::efs::OpenInode;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::convert::TryFrom;
use spin::Mutex;
/// Virtual filesystem layer over easy-fs
pub struct Inode {
    inode_id: u32,
    block_id: usize,
    block_offset: usize,
    fs: Arc<Mutex<EasyFileSystem>>,
    block_device: Arc<dyn BlockDevice>,
}

impl Inode {
    pub(crate) fn new(
        inode_id: u32,
        fs: &Arc<Mutex<EasyFileSystem>>,
        efs: &EasyFileSystem,
    ) -> Self {
        let (block_id, block_offset) = efs.get_disk_inode_pos(inode_id);
        Self {
            inode_id,
            block_id: block_id as usize,
            block_offset,
            fs: Arc::clone(fs),
            block_device: Arc::clone(&efs.block_device),
        }
    }

    fn open_inode(&self, inode_id: u32, fs: &mut EasyFileSystem) -> Arc<Inode> {
        fs.open_inodes
            .entry(inode_id)
            .or_insert(OpenInode {
                handles: 0,
                unlinked: false,
            })
            .handles += 1;
        Arc::new(Self::new(inode_id, &self.fs, fs))
    }

    /// Call a function over a disk inode to read it
    fn read_disk_inode<V>(&self, f: impl FnOnce(&DiskInode) -> V) -> V {
        get_block_cache(self.block_id, Arc::clone(&self.block_device))
            .lock()
            .read(self.block_offset, f)
    }
    /// Call a function over a disk inode to modify it
    fn modify_disk_inode<V>(&self, f: impl FnOnce(&mut DiskInode) -> V) -> V {
        get_block_cache(self.block_id, Arc::clone(&self.block_device))
            .lock()
            .modify(self.block_offset, f)
    }
    fn valid_name(name: &str) -> bool {
        !name.is_empty() && name.len() <= NAME_LENGTH_LIMIT && !name.contains('\0')
    }

    fn dir_entries<'a>(&'a self, disk_inode: &'a DiskInode) -> impl Iterator<Item = DirEntry> + 'a {
        assert!(disk_inode.is_dir());
        let file_count = disk_inode.size as usize / DIRENT_SZ;
        (0..file_count).map(move |index| {
            let mut dirent = DirEntry::empty();
            assert_eq!(
                disk_inode.read_at(index * DIRENT_SZ, dirent.as_bytes_mut(), &self.block_device),
                DIRENT_SZ,
            );
            dirent
        })
    }

    /// Find inode under current inode by name.
    pub fn find(&self, name: &str) -> Option<Arc<Inode>> {
        if !Self::valid_name(name) {
            return None;
        }
        let mut fs = self.fs.lock();
        let inode_id = self.read_disk_inode(|disk_inode| {
            self.dir_entries(disk_inode)
                .find(|entry| entry.name() == name)
                .map(|entry| entry.inode_id())
        })?;
        Some(self.open_inode(inode_id, &mut fs))
    }

    fn increase_size(
        &self,
        new_size: u32,
        disk_inode: &mut DiskInode,
        fs: &mut EasyFileSystem,
    ) -> Option<()> {
        if new_size <= disk_inode.size {
            return Some(());
        }
        if new_size as usize > INDIRECT2_BOUND * BLOCK_SZ {
            return None;
        }
        let mut blocks = Vec::new();
        for _ in 0..disk_inode.blocks_num_needed(new_size) {
            let Some(block) = fs.alloc_data() else {
                for allocated in blocks {
                    fs.dealloc_data(allocated);
                }
                return None;
            };
            blocks.push(block);
        }
        disk_inode.increase_size(new_size, blocks, &self.block_device);
        Some(())
    }

    fn insert_entry(&self, name: &str, inode_id: u32, fs: &mut EasyFileSystem) -> Option<()> {
        self.modify_disk_inode(|disk_inode| {
            let offset = self
                .dir_entries(disk_inode)
                .position(|entry| entry.name().is_empty())
                .map_or(disk_inode.size as usize, |index| index * DIRENT_SZ);
            let entry = DirEntry::new(name, inode_id);
            self.increase_size((offset + DIRENT_SZ) as u32, disk_inode, fs)?;
            disk_inode.write_at(offset, entry.as_bytes(), &self.block_device);
            Some(())
        })
    }

    /// Create inode under current inode by name.
    pub fn create(&self, name: &str) -> Option<Arc<Inode>> {
        if !Self::valid_name(name) {
            return None;
        }
        let mut fs = self.fs.lock();
        if self.read_disk_inode(|disk_inode| {
            self.dir_entries(disk_inode)
                .any(|entry| entry.name() == name)
        }) {
            return None;
        }
        let inode_id = fs.alloc_inode()?;
        if self.insert_entry(name, inode_id, &mut fs).is_none() {
            fs.inode_bitmap
                .dealloc(&self.block_device, inode_id as usize);
            return None;
        }
        let inode = self.open_inode(inode_id, &mut fs);
        inode.modify_disk_inode(|disk_inode| disk_inode.initialize(DiskInodeType::File));
        block_cache_sync_all();
        Some(inode)
    }

    /// Add a second name for an existing file in this directory.
    pub fn link(&self, old_name: &str, new_name: &str) -> Option<()> {
        if !Self::valid_name(old_name) || !Self::valid_name(new_name) {
            return None;
        }
        let mut fs = self.fs.lock();
        let inode_id = self.read_disk_inode(|disk_inode| {
            let mut inode_id = None;
            for entry in self.dir_entries(disk_inode) {
                if entry.name() == new_name {
                    return None;
                }
                if entry.name() == old_name {
                    inode_id = Some(entry.inode_id());
                }
            }
            inode_id
        })?;
        self.insert_entry(new_name, inode_id, &mut fs)?;
        block_cache_sync_all();
        Some(())
    }

    /// Remove a name and reclaim the inode when its last handle and name are gone.
    pub fn unlink(&self, name: &str) -> Option<()> {
        if !Self::valid_name(name) {
            return None;
        }
        let mut fs = self.fs.lock();
        let (inode_id, last_link) = self.modify_disk_inode(|disk_inode| {
            let (index, entry) = self
                .dir_entries(disk_inode)
                .enumerate()
                .find(|(_, entry)| entry.name() == name)?;
            let inode_id = entry.inode_id();
            disk_inode.write_at(
                index * DIRENT_SZ,
                DirEntry::empty().as_bytes(),
                &self.block_device,
            );
            let last_link = !self
                .dir_entries(disk_inode)
                .any(|entry| entry.inode_id() == inode_id);
            Some((inode_id, last_link))
        })?;
        if last_link {
            if let Some(open) = fs.open_inodes.get_mut(&inode_id) {
                open.unlinked = true;
            } else {
                Self::reclaim_inode(&mut fs, inode_id);
            }
        }
        block_cache_sync_all();
        Some(())
    }

    /// Return inode number and the number of directory entries that name this file.
    pub fn metadata(&self, inode: &Inode) -> (u32, u32) {
        let _fs = self.fs.lock();
        let nlink = self.read_disk_inode(|disk_inode| {
            self.dir_entries(disk_inode)
                .filter(|entry| entry.inode_id() == inode.inode_id)
                .count()
        });
        (inode.inode_id, nlink as u32)
    }

    fn reclaim_inode(fs: &mut EasyFileSystem, inode_id: u32) {
        let (block_id, offset) = fs.get_disk_inode_pos(inode_id);
        let blocks = get_block_cache(block_id as usize, Arc::clone(&fs.block_device))
            .lock()
            .modify(offset, |inode: &mut DiskInode| {
                inode.clear_size(&fs.block_device)
            });
        for block in blocks {
            fs.dealloc_data(block);
        }
        fs.inode_bitmap.dealloc(&fs.block_device, inode_id as usize);
    }

    /// List inodes under current inode
    pub fn ls(&self) -> Vec<String> {
        let _fs = self.fs.lock();
        self.read_disk_inode(|disk_inode| {
            self.dir_entries(disk_inode)
                .filter(|entry| !entry.name().is_empty())
                .map(|entry| String::from(entry.name()))
                .collect()
        })
    }
    /// Read data from current inode
    pub fn read_at(&self, offset: usize, buf: &mut [u8]) -> usize {
        let _fs = self.fs.lock();
        self.read_disk_inode(|disk_inode| disk_inode.read_at(offset, buf, &self.block_device))
    }
    /// Write data to current inode
    pub fn write_at(&self, offset: usize, buf: &[u8]) -> usize {
        let mut fs = self.fs.lock();
        let size = self.modify_disk_inode(|disk_inode| {
            let Some(end) = offset
                .checked_add(buf.len())
                .and_then(|end| u32::try_from(end).ok())
            else {
                return 0;
            };
            if self.increase_size(end, disk_inode, &mut fs).is_none() {
                return 0;
            }
            disk_inode.write_at(offset, buf, &self.block_device)
        });
        block_cache_sync_all();
        size
    }
    /// Clear the data in current inode
    pub fn clear(&self) {
        let mut fs = self.fs.lock();
        self.modify_disk_inode(|disk_inode| {
            let size = disk_inode.size;
            let data_blocks_dealloc = disk_inode.clear_size(&self.block_device);
            assert!(data_blocks_dealloc.len() == DiskInode::total_blocks(size) as usize);
            for data_block in data_blocks_dealloc.into_iter() {
                fs.dealloc_data(data_block);
            }
        });
        block_cache_sync_all();
    }
}

impl Drop for Inode {
    fn drop(&mut self) {
        if self.inode_id == 0 {
            return;
        }
        let mut fs = self.fs.lock();
        let open = fs.open_inodes.get_mut(&self.inode_id).unwrap();
        open.handles -= 1;
        if open.handles == 0 {
            let unlinked = open.unlinked;
            fs.open_inodes.remove(&self.inode_id);
            if unlinked {
                Self::reclaim_inode(&mut fs, self.inode_id);
                block_cache_sync_all();
            }
        }
    }
}
