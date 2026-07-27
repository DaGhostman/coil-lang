// examples/nested_aggregates.hy — arrays of typed tuples (and aliases).
//
// Shows how to compose aggregates into richer data shapes:
//   type Row = (string, int);
//   type Table = [Row];
//
// Expected output: `alice:30bob:25total:55`

type Row = (string, int);
type Table = [Row];

fn sum_ages(Table rows) -> int {
    let total = 0;
    for row in rows {
        let (name, age) = row;
        print "%s:%i", name, age;
        total = total + age;
    }
    return total;
}

fn main() {
    let people: Table = [("alice", 30), ("bob", 25)];
    print "total:%i", sum_ages(people);
}
