use crate::utils::open_file;
use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt::Debug;
use std::fs::File;
use std::io::{BufRead, BufReader, Error, ErrorKind, Read, Result, Write};
use std::path::Path;

/// 解析 ncbi 文件的 taxonomy nodes 文件
pub fn parse_nodes_file<P: AsRef<Path>>(
    nodes_filename: P,
) -> Result<(
    HashMap<u64, u64>,
    HashMap<u64, HashSet<u64>>,
    HashMap<u64, String>,
    HashSet<String>,
)> {
    let nodes_file = open_file(nodes_filename)?;
    let reader = BufReader::new(nodes_file);

    let mut parent_map = HashMap::new();
    let mut child_map = HashMap::new();
    let mut rank_map = HashMap::new();
    let mut known_ranks = HashSet::new();

    for line in reader.lines() {
        let line = line?;
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let fields: Vec<_> = line.split("\t|\t").collect();
        if fields.len() < 3 {
            continue;
        }

        let node_id = fields[0]
            .parse::<u64>()
            .map_err(|_| Error::new(ErrorKind::InvalidData, "node_id"))?;

        let parent_id = if node_id == 1 {
            0
        } else {
            fields[1]
                .parse::<u64>()
                .map_err(|_| Error::new(ErrorKind::InvalidData, "parent_id"))?
        };

        let rank = fields[2].to_string();

        parent_map.insert(node_id, parent_id);
        child_map
            .entry(parent_id)
            .or_insert_with(HashSet::new)
            .insert(node_id);
        rank_map.insert(node_id, rank.clone());
        known_ranks.insert(rank);
    }

    Ok((parent_map, child_map, rank_map, known_ranks))
}

/// 解析 ncbi 文件的 taxonomy names 文件
pub fn parse_names_file<P: AsRef<Path>>(names_filename: P) -> Result<HashMap<u64, String>> {
    let names_file = open_file(names_filename)?;
    let reader = BufReader::new(names_file);

    let mut name_map = HashMap::new();

    for line in reader.lines() {
        let line = line?;
        // 忽略空行或注释行
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.trim_end_matches(|c| c == '\t' || c == '|' || c == '\n');
        // 分割行为字段
        let fields: Vec<_> = line.split("\t|\t").collect();
        if fields.len() < 4 {
            continue; // 如果不满足预期的字段数量，则跳过此行
        }
        // 解析节点 ID 和名称类型
        let node_id = fields[0].parse::<u64>().unwrap_or(0);
        let name = fields[1].to_string();
        let name_type = fields[3].to_string();

        // 仅当类型为 "scientific name" 时，将名称添加到 map 中
        if name_type == "scientific name" {
            name_map.insert(node_id, name);
        }
    }

    Ok(name_map)
}

/// 结构体定义
#[derive(Debug)]
pub struct TaxonomyNode {
    pub parent_id: u64,
    pub first_child: u64,
    pub child_count: u64,
    pub name_offset: u64,
    pub rank_offset: u64,
    pub external_id: u64,
    pub godparent_id: u64,
}

impl Default for TaxonomyNode {
    fn default() -> Self {
        Self {
            parent_id: 0,
            first_child: 0,
            child_count: 0,
            name_offset: 0,
            rank_offset: 0,
            external_id: 0,
            godparent_id: 0,
        }
    }
}

// NCBITaxonomy 类型定义
pub struct NCBITaxonomy {
    parent_map: HashMap<u64, u64>,
    name_map: HashMap<u64, String>,
    rank_map: HashMap<u64, String>,
    child_map: HashMap<u64, HashSet<u64>>,
    marked_nodes: HashSet<u64>,
    known_ranks: HashSet<String>,
}

impl NCBITaxonomy {
    // 构造函数等实现
    pub fn from_ncbi<P: AsRef<Path>>(nodes_filename: P, names_filename: P) -> Result<Self> {
        let mut marked_nodes = HashSet::new();
        let (parent_map, child_map, rank_map, known_ranks) = parse_nodes_file(nodes_filename)?;

        let name_map = parse_names_file(names_filename)?;

        marked_nodes.insert(1); // 标记根节点

        Ok(NCBITaxonomy {
            parent_map,
            name_map,
            rank_map,
            child_map,
            known_ranks,
            marked_nodes,
        })
    }

    pub fn mark_node(&mut self, taxid: u64) {
        let mut current_taxid = taxid;
        while !self.marked_nodes.contains(&current_taxid) {
            self.marked_nodes.insert(current_taxid);
            if let Some(&parent_id) = self.parent_map.get(&current_taxid) {
                current_taxid = parent_id;
            } else {
                break;
            }
        }
    }

    pub fn get_rank_offset_data(&self) -> (HashMap<String, u64>, String) {
        let mut rank_data = String::new();
        let mut rank_offsets = HashMap::new();

        let mut known_ranks: Vec<_> = self.known_ranks.iter().collect();
        known_ranks.sort_unstable();

        for rank in known_ranks {
            rank_offsets.insert(rank.clone(), rank_data.len() as u64);
            rank_data.push_str(rank);
            rank_data.push('\0');
        }

        (rank_offsets, rank_data)
    }

    pub fn convert_to_kraken_taxonomy(&self) -> Taxonomy {
        let mut taxo = Taxonomy::default();
        // 预分配内存
        taxo.nodes.reserve(self.marked_nodes.len() + 1);
        taxo.nodes.push(TaxonomyNode::default());

        let mut name_data = String::new();
        let (rank_offsets, rank_data) = self.get_rank_offset_data();

        let mut bfs_queue = VecDeque::new();
        bfs_queue.push_back(1);
        let mut external_id_map = HashMap::new();
        external_id_map.insert(0, 0);
        external_id_map.insert(1, 1);
        let mut internal_node_id = 0;

        while let Some(external_node_id) = bfs_queue.pop_front() {
            internal_node_id += 1;
            external_id_map.insert(external_node_id, internal_node_id);

            let mut node = TaxonomyNode::default();
            node.parent_id = external_id_map
                .get(&self.parent_map[&external_node_id])
                .unwrap()
                .clone();
            node.external_id = external_node_id;
            node.rank_offset = *rank_offsets.get(&self.rank_map[&external_node_id]).unwrap();
            node.name_offset = name_data.len() as u64;

            let name = self.name_map.get(&external_node_id).unwrap();
            name_data.push_str(name);
            name_data.push('\0');

            node.first_child = internal_node_id + bfs_queue.len() as u64 + 1;

            if let Some(children) = self.child_map.get(&external_node_id) {
                let mut sorted_children: Vec<_> = children.iter().collect();
                sorted_children.sort_unstable();

                for &child_node in sorted_children {
                    if self.marked_nodes.contains(&child_node) {
                        bfs_queue.push_back(child_node);
                        node.child_count += 1;
                    }
                }
            }
            taxo.nodes.push(node);
        }

        taxo.name_data = name_data.into_bytes();
        taxo.rank_data = rank_data.into_bytes();

        taxo
    }
}

// Taxonomy 类型定义
#[derive(Debug)]
pub struct Taxonomy {
    pub path_cache: HashMap<u32, Vec<u32>>,
    pub nodes: Vec<TaxonomyNode>,
    pub name_data: Vec<u8>, // 字符串数据以 Vec<u8> 存储
    pub rank_data: Vec<u8>, // 字符串数据以 Vec<u8> 存储
    external_to_internal_id_map: HashMap<u64, u32>,
}

impl Default for Taxonomy {
    fn default() -> Self {
        Taxonomy {
            path_cache: HashMap::new(),
            nodes: Vec::new(),
            name_data: Vec::new(),
            rank_data: Vec::new(),
            external_to_internal_id_map: HashMap::new(),
        }
    }
}

impl Taxonomy {
    const MAGIC: &'static [u8] = b"K2TAXDAT"; // 替换为实际的 magic bytes

    pub fn from_file<P: AsRef<Path> + Debug>(filename: P) -> Result<Taxonomy> {
        let mut file = open_file(&filename)?;

        let mut magic = vec![0; Self::MAGIC.len()];
        file.read_exact(&mut magic)?;
        if magic != Self::MAGIC {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("Malformed taxonomy file {:?}", &filename),
            ));
        }

        let mut buffer = [0; 24];
        file.read_exact(&mut buffer)?;
        let (node_count, name_data_len, rank_data_len) =
            unsafe { std::mem::transmute::<[u8; 24], (u64, u64, u64)>(buffer) };

        let mut nodes = Vec::with_capacity(node_count as usize);
        for _ in 0..node_count {
            let mut buffer = [0; 56];
            file.read_exact(&mut buffer)?;
            let node = unsafe { std::mem::transmute::<[u8; 56], TaxonomyNode>(buffer) };
            nodes.push(node);
        }

        let mut name_data = vec![0; name_data_len as usize];
        file.read_exact(&mut name_data)?;

        let mut rank_data = vec![0; rank_data_len as usize];
        file.read_exact(&mut rank_data)?;

        let mut external_to_internal_id_map = HashMap::new();
        for (internal_id, node) in nodes.iter().enumerate() {
            let external_id = node.external_id;
            external_to_internal_id_map.insert(external_id, internal_id as u32);
        }

        let mut taxo = Taxonomy {
            path_cache: HashMap::new(),
            nodes,
            name_data,
            rank_data,
            external_to_internal_id_map,
        };
        taxo.build_path_cache();
        Ok(taxo)
    }

    pub fn _is_a_ancestor_of_b(&self, a: u32, b: u32) -> bool {
        if a == 0 || b == 0 {
            return false;
        }

        let mut current = b;

        while current > a {
            current = match self.nodes.get(current as usize) {
                Some(node) => node.parent_id as u32,
                None => return false,
            };
        }

        current == a
    }

    pub fn is_a_ancestor_of_b(&self, a: u32, b: u32) -> bool {
        if a == 0 || b == 0 {
            return false;
        }

        // 尝试从path_cache中获取b的祖先路径
        if let Some(path) = self.path_cache.get(&b) {
            // 检查路径中是否包含a
            return path.contains(&a);
        }

        false
    }

    // 查找两个节点的最低公共祖先
    pub fn lca(&self, a: u32, b: u32) -> u32 {
        if a == 0 || b == 0 || a == b {
            return if a != 0 { a } else { b };
        }

        let default: Vec<u32> = vec![0];
        let path_a = self.path_cache.get(&a).unwrap_or(&default);
        let path_b = self.path_cache.get(&b).unwrap_or(&default);

        let mut i = 0;
        while i < path_a.len() && i < path_b.len() && path_a[i] == path_b[i] {
            i += 1;
        }

        if i == 0 {
            return 0;
        }

        // 返回最后一个共同的祖先
        *path_a.get(i - 1).unwrap_or(&0)
    }

    pub fn lowest_common_ancestor(&self, mut a: u32, mut b: u32) -> u32 {
        // 如果任何一个节点是 0，返回另一个节点
        if a == 0 || b == 0 || a == b {
            return if a != 0 { a } else { b };
        }

        // 遍历节点直到找到共同的祖先
        while a != b {
            if a > b {
                a = self
                    .nodes
                    .get(a as usize)
                    .map_or(0, |node| node.parent_id as u32);
            } else {
                b = self
                    .nodes
                    .get(b as usize)
                    .map_or(0, |node| node.parent_id as u32);
            }
        }

        a
    }

    pub fn build_path_cache(&mut self) {
        let mut cache: HashMap<u32, Vec<u32>> = HashMap::new();
        let root_external_id = 1u64;
        if let Some(&root_internal_id) = self.external_to_internal_id_map.get(&root_external_id) {
            // 开始从根节点遍历
            self.build_path_for_node(root_internal_id, &mut cache, Vec::new());
        }
        self.path_cache = cache;
    }

    fn build_path_for_node(
        &self,
        node_id: u32,
        path_cache: &mut HashMap<u32, Vec<u32>>,
        mut current_path: Vec<u32>,
    ) {
        current_path.push(node_id); // 将当前节点添加到路径中
                                    // 存储当前节点的路径
        path_cache.insert(node_id, current_path.clone());

        // 获取当前节点的信息
        let node = &self.nodes[node_id as usize];
        let first_child_id = node.first_child as u32;
        let child_count = node.child_count as u32;

        // 遍历所有子节点
        for i in 0..child_count {
            let child_internal_id = first_child_id + i; // 这里假设子节点的ID是连续的
            self.build_path_for_node(child_internal_id, path_cache, current_path.clone());
        }
    }

    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    // get_internal_id 函数的优化
    pub fn get_internal_id(&self, external_id: u64) -> u32 {
        *self
            .external_to_internal_id_map
            .get(&external_id)
            .unwrap_or(&0)
    }

    pub fn generate_external_to_internal_id_map(&mut self) {
        self.external_to_internal_id_map.clear();
        self.external_to_internal_id_map.insert(0, 0);

        for (i, node) in self.nodes.iter().enumerate() {
            self.external_to_internal_id_map
                .insert(node.external_id, i as u32);
        }
    }

    pub fn write_to_disk<P: AsRef<Path>>(&self, filename: P) -> Result<()> {
        let mut file = File::create(filename)?;

        // Write file magic
        file.write_all(Taxonomy::MAGIC)?;

        // Write node count, name data length, and rank data length
        let node_count = self.nodes.len() as u64;
        let name_data_len = self.name_data.len() as u64;
        let rank_data_len = self.rank_data.len() as u64;
        file.write_all(&node_count.to_le_bytes())?;
        file.write_all(&name_data_len.to_le_bytes())?;
        file.write_all(&rank_data_len.to_le_bytes())?;

        // Write nodes as binary data
        for node in &self.nodes {
            file.write_all(&node.parent_id.to_le_bytes())?;
            file.write_all(&node.first_child.to_le_bytes())?;
            file.write_all(&node.child_count.to_le_bytes())?;
            file.write_all(&node.name_offset.to_le_bytes())?;
            file.write_all(&node.rank_offset.to_le_bytes())?;
            file.write_all(&node.external_id.to_le_bytes())?;
            file.write_all(&node.godparent_id.to_le_bytes())?;
        }

        // Write name data and rank data
        file.write_all(&self.name_data)?;
        file.write_all(&self.rank_data)?;

        Ok(())
    }
}
