// examples/collections_map.hy — HashMap insert / get_or / update
//
// Output: A,2

use io::{stdout};
use io::sync::{write_all};
use string::{format, to_bytes};
use collections::map::{hashmap_new, hashmap_insert, hashmap_get_or};

fn main() {
    let m = hashmap_new();
    hashmap_insert(m, 1, "a");
    hashmap_insert(m, 2, "b");
    hashmap_insert(m, 1, "A");
    write_all(stdout(), to_bytes(format("%s,%i", hashmap_get_or(m, 1, "?"), m.size())));
}
