use super::*;
use nalgebra::DMatrix;

fn p3_adjacency() -> DMatrix<f64> {
    DMatrix::from_row_slice(3, 3, &[0.0, 1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0, 0.0])
}

#[test]
fn normalized_laplacian_empty_graph() {
    let w = DMatrix::<f64>::zeros(0, 0);
    let l = normalized_laplacian(&w);
    assert_eq!(l.nrows(), 0);
    assert_eq!(l.ncols(), 0);
}

#[test]
fn normalized_laplacian_single_node() {
    // n=1: isolated node with zero degree; diagonal must be 1.0 (convention)
    let w = DMatrix::<f64>::zeros(1, 1);
    let l = normalized_laplacian(&w);
    assert!((l[(0, 0)] - 1.0).abs() < 1e-12);
}

#[test]
fn normalized_laplacian_isolated_degree_zero_node() {
    // 3 nodes: 0-1 connected, node 2 is degree-0; its row/col must have 1.0 on diagonal only
    let w = DMatrix::from_row_slice(3, 3, &[0.0, 1.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
    let l = normalized_laplacian(&w);
    assert!((l[(2, 2)] - 1.0).abs() < 1e-12);
    assert!(l[(2, 0)].abs() < 1e-12);
    assert!(l[(2, 1)].abs() < 1e-12);
    assert!(l[(0, 2)].abs() < 1e-12);
    assert!(l[(1, 2)].abs() < 1e-12);
}

#[test]
fn fiedler_vector_degenerate_all_isolated() {
    // All-zero adjacency → normalized Laplacian = identity → all eigenvalues = 1.0;
    // fiedler_vector must not panic and must return a unit vector.
    let w = DMatrix::<f64>::zeros(3, 3);
    let l = normalized_laplacian(&w);
    let fv = fiedler_vector(&l).unwrap();
    let norm = fv.norm();
    assert!(
        (norm - 1.0).abs() < 1e-10,
        "eigenvector must be unit, got norm={norm}"
    );
}

#[test]
fn fiedler_vector_is_second_smallest_eigenvector() {
    // P3 normalized Laplacian has eigenvalues 0, 1, 2.
    // After the fix, fiedler_vector must return the eigenvector for lambda_2=1.
    // Verify: L * v ≈ 1.0 * v  (residual < 1e-10).
    let w = p3_adjacency();
    let l = normalized_laplacian(&w);
    let fv = fiedler_vector(&l).unwrap();
    let lv = &l * &fv;
    let residual = (&lv - &fv).norm(); // L*v - lambda_2*v, lambda_2=1 so subtract fv directly
    assert!(
        residual < 1e-10,
        "fiedler_vector did not return the 2nd-smallest eigenvector; L*v - v residual={residual}"
    );
}
