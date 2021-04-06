use crate::collections::skip_list::skipmap::{IntoIter, SkipMap};
use crate::sstable::level0_table::Level0Manager;
use crate::sstable::manager::TableManager;
use crate::sstable::table_handle::TableReadHandle;
use std::cmp::Ordering;
use std::collections::VecDeque;
use std::sync::Arc;

pub const LEVEL0_FILES_THRESHOLD: usize = 4;

/// Merge all the `level0_table_handles` and `level1_tables` to `new_table`,
/// then insert `new_table` to `TableManager`.
pub(crate) fn compact_and_insert(
    level0_manager: &Arc<Level0Manager>,
    table_manager: &Arc<TableManager>,
    level0_table_handles: Vec<Arc<TableReadHandle>>,
    level1_table_handles: VecDeque<Arc<TableReadHandle>>,
) {
    let level0_skip_map = merge_level0_tables(&level0_table_handles);

    if level1_table_handles.is_empty() {
        let level1_table_size = level0_skip_map.len() / LEVEL0_FILES_THRESHOLD;
        if level1_table_size == 0 {
            // create only one level1 table
            let mut new_table = table_manager.create_table_write_handle(1);
            new_table.write_sstable(&level0_skip_map).unwrap();
            table_manager.upsert_table_handle(new_table);
        } else {
            let level0_kvs = level0_skip_map.iter();
            let mut temp_kvs = vec![];
            for kv in level0_kvs {
                unsafe {
                    temp_kvs.push((&(*kv).entry.key, &(*kv).entry.value));
                }
                if temp_kvs.len() % level1_table_size == 0 {
                    add_table_handle_from_vec_ref(temp_kvs, table_manager);
                    temp_kvs = vec![];
                }
            }
            if !temp_kvs.is_empty() {
                add_table_handle_from_vec_ref(temp_kvs, table_manager);
            }
        }
    } else {
        let mut kv_total = level0_skip_map.len() as u64;
        for table in &level1_table_handles {
            kv_total += table.kv_total() as u64;
        }

        let level1_table_size = kv_total / LEVEL0_FILES_THRESHOLD as u64;
        debug_assert!(level1_table_size > 0);

        let mut level0_iter: IntoIter<String, String> = level0_skip_map.into_iter();
        let mut temp_kvs = vec![];

        let mut kv = level0_iter.current_mut_no_consume();
        for level1_table_handle in level1_table_handles.iter() {
            for (level1_key, level1_value) in level1_table_handle.iter() {
                if level1_key == "key300" && level1_table_handle.table_id() == 6 {
                    println!("old level1: {} key300", level1_table_handle.table_id());
                }
                if kv.is_null() {
                    // write all the remain key-values in level1 tables.
                    temp_kvs.push((level1_key, level1_value));
                    if temp_kvs.len() as u64 % level1_table_size == 0 {
                        add_table_handle_from_vec(temp_kvs, table_manager);
                        temp_kvs = vec![];
                    }
                } else {
                    loop {
                        let level0_key = unsafe { &(*kv).entry.key };
                        debug_assert!(!level0_key.is_empty());
                        match level0_key.cmp(&level1_key) {
                            // set to level0_value
                            // drop level1_value
                            Ordering::Equal => {
                                let level0_entry = unsafe { std::mem::take(&mut (*kv).entry) };
                                let (level0_key, level0_value) = level0_entry.key_value();
                                temp_kvs.push((level0_key, level0_value));
                                if temp_kvs.len() as u64 % level1_table_size == 0 {
                                    add_table_handle_from_vec(temp_kvs, table_manager);
                                    temp_kvs = vec![];
                                }
                                kv = level0_iter.next_node();
                                break;
                            }
                            // insert level1_value
                            Ordering::Greater => {
                                temp_kvs.push((level1_key, level1_value));
                                if temp_kvs.len() as u64 % level1_table_size == 0 {
                                    add_table_handle_from_vec(temp_kvs, table_manager);
                                    temp_kvs = vec![];
                                }
                                break;
                            }
                            // insert level0_value
                            Ordering::Less => {
                                let level0_entry = unsafe { std::mem::take(&mut (*kv).entry) };
                                let (level0_key, level0_value) = level0_entry.key_value();
                                temp_kvs.push((level0_key, level0_value));
                                if temp_kvs.len() as u64 % level1_table_size == 0 {
                                    add_table_handle_from_vec(temp_kvs, table_manager);
                                    temp_kvs = vec![];
                                }
                                kv = level0_iter.next_node();
                                if kv.is_null() {
                                    temp_kvs.push((level1_key, level1_value));
                                    if temp_kvs.len() as u64 % level1_table_size == 0 {
                                        add_table_handle_from_vec(temp_kvs, table_manager);
                                        temp_kvs = vec![];
                                    }
                                    break;
                                }
                            }
                        }
                    }
                }
            }
        }

        // write all the remain kv in level0 tables.
        while !kv.is_null() {
            unsafe {
                let entry = std::mem::take(&mut (*kv).entry);
                temp_kvs.push((entry.key, entry.value));
                if temp_kvs.len() as u64 % level1_table_size == 0 {
                    add_table_handle_from_vec(temp_kvs, table_manager);
                    temp_kvs = vec![];
                }
            }
            kv = level0_iter.next_node();
        }

        if !temp_kvs.is_empty() {
            add_table_handle_from_vec(temp_kvs, table_manager);
        }
    }

    for table in level0_table_handles {
        level0_manager.ready_to_delete(table.table_id());
    }
    for table in level1_table_handles {
        table_manager.ready_to_delete(table);
    }
}

fn add_table_handle_from_vec(temp_kvs: Vec<(String, String)>, table_manager: &Arc<TableManager>) {
    if !temp_kvs.is_empty() {
        let mut new_table = table_manager.create_table_write_handle(1);

        for (k, v) in temp_kvs.iter() {
            if k == "key300" {
                println!("{} {} {}", k, v, new_table.table_id());
                break;
            }
        }

        new_table.write_sstable_from_vec(temp_kvs).unwrap();
        table_manager.upsert_table_handle(new_table);
    }
}

fn add_table_handle_from_vec_ref(
    temp_kvs: Vec<(&String, &String)>,
    table_manager: &Arc<TableManager>,
) {
    if !temp_kvs.is_empty() {
        let mut new_table = table_manager.create_table_write_handle(1);
        new_table.write_sstable_from_vec_ref(temp_kvs).unwrap();
        table_manager.upsert_table_handle(new_table);
    }
}

fn merge_level0_tables(level0_tables: &[Arc<TableReadHandle>]) -> SkipMap<String, String> {
    let mut skip_map = SkipMap::new();
    for table in level0_tables {
        for (key, value) in table.iter() {
            skip_map.insert(key, value);
        }
    }
    skip_map
}
