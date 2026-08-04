// examples/nested_aggregates.hy — arrays of typed tuples (and aliases).
//
// Shows how to compose aggregates into richer data shapes:
//   type Row = (string, int);
//   type Table = [Row];
//
// Expected output: `alice:30bob:25total:55`

use io::{stdout};
use io::sync::{write_all};
use string::{format, to_bytes};
type Row = (string, int);
type Table = [Row];

fn sum_ages(Table rows) -> int {
    let total = 0;
    for row in rows {
        let (name, age) = row;
        write_all(stdout(), to_bytes(format("%s:%i", name, age)));
        total = total + age;
    }
    return total;
}

fn main() {
    let people: Table = [("alice", 30), ("bob", 25)];
    write_all(stdout(), to_bytes(format("total:%i", sum_ages(people))));
}
