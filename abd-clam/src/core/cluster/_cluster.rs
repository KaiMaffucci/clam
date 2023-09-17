//! The `Cluster` is the heart of CLAM. It provides the ability to perform a
//! divisive hierarchical cluster of arbitrary datasets in arbitrary metric
//! spaces.

use core::{
    hash::{Hash, Hasher},
    marker::PhantomData,
};
use distances::Number;

use crate::{utils, Dataset, PartitionCriteria, PartitionCriterion};

/// Ratios are used for anomaly detection and related applications.
pub type Ratios = [f64; 6];

/// A `Cluster` represents a collection of "similar" instances from a metric-`Space`.
///
/// `Cluster`s can be unwieldy to use directly unless one has a good grasp of
/// the underlying invariants. We anticipate that most users' needs will be well
/// met by the higher-level abstractions, e.g. `Tree`, `Graph`, `CAKES`, etc.
///
/// For now, `Cluster` names are unique within a single tree. We plan on adding
/// tree-based prefixes which will make names unique across multiple trees.
#[derive(Debug)]
pub struct Cluster<T: Send + Sync + Copy, U: Number> {
    /// The `Cluster`'s history in the tree.
    pub history: Vec<bool>,
    /// The seed used in the random number generator for this `Cluster`.
    pub seed: Option<u64>,
    /// The offset of the indices of the `Cluster`'s instances in the dataset.
    pub offset: usize,
    /// The number of instances in the `Cluster`.
    pub cardinality: usize,
    /// The index of the `center` instance in the dataset.
    pub arg_center: usize,
    /// The index of the `radial` instance in the dataset.
    pub arg_radial: usize,
    /// The distance from the `center` to the `radial` instance.
    pub radius: U,
    /// The local fractal dimension of the `Cluster`.
    #[allow(dead_code)]
    pub lfd: f64,
    /// The children of the `Cluster`.
    pub children: Option<Children<T, U>>,
    /// The six `Cluster` ratios used for anomaly detection and related applications.
    #[allow(dead_code)]
    pub ratios: Option<Ratios>,
}

/// The children of a `Cluster`.
#[derive(Debug)]
pub struct Children<T: Send + Sync + Copy, U: Number> {
    /// The left child of the `Cluster`.
    pub left: Box<Cluster<T, U>>,
    /// The right child of the `Cluster`.
    pub right: Box<Cluster<T, U>>,
    /// The left pole of the `Cluster` (i.e. the instance used to identify
    /// instances for the left child).
    pub arg_l: usize,
    /// The right pole of the `Cluster` (i.e. the instance used to identify
    /// instances for the right child).
    pub arg_r: usize,
    /// The distance from the `l_pole` to the `r_pole` instance.
    pub polar_distance: U,
    /// To satisfy the compiler.
    _t: PhantomData<T>,
}

impl<T: Send + Sync + Copy, U: Number> PartialEq for Cluster<T, U> {
    fn eq(&self, other: &Self) -> bool {
        self.history == other.history
    }
}

impl<T: Send + Sync + Copy, U: Number> Eq for Cluster<T, U> {}

impl<T: Send + Sync + Copy, U: Number> PartialOrd for Cluster<T, U> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        if self.depth() == other.depth() {
            self.offset.partial_cmp(&other.offset)
        } else {
            self.depth().partial_cmp(&other.depth())
        }
    }
}

impl<T: Send + Sync + Copy, U: Number> Ord for Cluster<T, U> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        if self.depth() == other.depth() {
            self.offset.cmp(&other.offset)
        } else {
            self.depth().cmp(&other.depth())
        }
    }
}

impl<T: Send + Sync + Copy, U: Number> Hash for Cluster<T, U> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        (self.offset, self.cardinality).hash(state);
    }
}

impl<T: Send + Sync + Copy, U: Number> std::fmt::Display for Cluster<T, U> {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}", self.name())
    }
}

impl<T: Send + Sync + Copy, U: Number> Cluster<T, U> {
    /// Creates a new root `Cluster`.
    ///
    /// # Arguments
    ///
    /// * `data`: on which to create the `Cluster`.
    /// * `indices`: The indices of instances from the `dataset` that are contained in the `Cluster`.
    /// * `seed`: The seed used in the random number generator for this `Cluster`.
    pub fn new_root<D: Dataset<T, U>>(data: &D, indices: &[usize], seed: Option<u64>) -> Self {
        Self::new(data, seed, vec![true], 0, indices)
    }

    /// Creates a new `Cluster`.
    ///
    /// # Arguments
    ///
    /// * `data`: on which to create the `Cluster`.
    /// * `seed`: The seed used in the random number generator for this `Cluster`.
    /// * `history`: The `Cluster`'s history in the tree.
    /// * `offset`: The offset of the indices of the `Cluster`'s instances in the dataset.
    /// * `indices`: The indices of instances from the `dataset` that are contained in the `Cluster`.
    pub fn new<D: Dataset<T, U>>(
        data: &D,
        seed: Option<u64>,
        history: Vec<bool>,
        offset: usize,
        indices: &[usize],
    ) -> Self {
        let cardinality = indices.len();

        // TODO: Explore with different values for the threshold e.g. 10, 100, 1000, etc.
        let arg_samples = if cardinality < 100 {
            indices.to_vec()
        } else {
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let n = (indices.len().as_f64().sqrt()) as usize;
            data.choose_unique(n, indices, seed)
        };

        let Some(arg_center) = data.median(&arg_samples) else {
            unreachable!("The cluster should have at least one instance.")
        };

        let center_distances = data.one_to_many(arg_center, indices);
        let Some((arg_radial, radius)) = utils::arg_max(&center_distances) else {
            unreachable!("The cluster should have at least one instance.")
        };
        let arg_radial = indices[arg_radial];

        let lfd = utils::compute_lfd(radius, &center_distances);

        Self {
            history,
            seed,
            offset,
            cardinality,
            arg_center,
            arg_radial,
            radius,
            lfd,
            children: None,
            ratios: None,
        }
    }

    /// Partitions the `Cluster` into two children if the `Cluster` meets the
    /// given `PartitionCriteria`.
    ///
    /// This method should only be called on a root `Cluster`. It is user error
    /// to call this method on a non-root `Cluster`.
    ///
    /// # Arguments
    ///
    /// * `data`: The `Dataset` for the `Cluster`.
    /// * `criteria`: The `PartitionCriteria` to use for partitioning.
    ///
    /// # Returns
    ///
    /// * The `Cluster` on which the method was called after partitioning
    /// recursively until the `PartitionCriteria` is no longer met on any of the
    /// leaf `Cluster`s.
    pub fn partition<D: Dataset<T, U>>(mut self, data: &mut D, criteria: &PartitionCriteria<T, U>) -> Self {
        let mut indices = data.indices().to_vec();
        (self, indices) = self._partition(data, criteria, indices);
        data.reorder(&indices);

        self
    }

    /// Recursive helper function for `partition`.
    fn _partition<D: Dataset<T, U>>(
        mut self,
        data: &D,
        criteria: &PartitionCriteria<T, U>,
        mut indices: Vec<usize>,
    ) -> (Self, Vec<usize>) {
        if criteria.check(&self) {
            let ([(arg_l, l_indices), (arg_r, r_indices)], polar_distance) = self.partition_once(data, indices);

            let r_offset = self.offset + l_indices.len();

            let ((left, l_indices), (right, r_indices)) = rayon::join(
                || {
                    Self::new(data, self.seed, self.child_history(false), self.offset, &l_indices)
                        ._partition(data, criteria, l_indices)
                },
                || {
                    Self::new(data, self.seed, self.child_history(true), r_offset, &r_indices)
                        ._partition(data, criteria, r_indices)
                },
            );

            let arg_l = utils::pos_val(&l_indices, arg_l)
                .map_or_else(|| unreachable!("We know the left pole is in the indices."), |(i, _)| i);
            let arg_r = utils::pos_val(&r_indices, arg_r)
                .map_or_else(|| unreachable!("We know the right pole is in the indices."), |(i, _)| i);

            self.children = Some(Children {
                left: Box::new(left),
                right: Box::new(right),
                arg_l: self.offset + arg_l,
                arg_r: r_offset + arg_r,
                polar_distance,
                _t: PhantomData,
            });

            indices = l_indices.into_iter().chain(r_indices).collect::<Vec<_>>();
        }

        // reset the indices to center and radial indices for data reordering
        let arg_center = utils::pos_val(&indices, self.arg_center)
            .map_or_else(|| unreachable!("We know the center is in the indices."), |(i, _)| i);
        self.arg_center = self.offset + arg_center;

        let arg_radial = utils::pos_val(&indices, self.arg_radial)
            .map_or_else(|| unreachable!("We know the radial is in the indices."), |(i, _)| i);
        self.arg_radial = self.offset + arg_radial;

        (self, indices)
    }

    /// Partitions the `Cluster` into two children once.
    fn partition_once<D: Dataset<T, U>>(&self, data: &D, indices: Vec<usize>) -> ([(usize, Vec<usize>); 2], U) {
        let l_distances = data.one_to_many(self.arg_radial, &indices);

        let Some((arg_r, polar_distance)) = utils::arg_max(&l_distances) else {
            unreachable!("The cluster should have at least one instance.")
        };
        let arg_r = indices[arg_r];
        let r_distances = data.one_to_many(arg_r, &indices);

        let (l_indices, r_indices) = indices
            .into_iter()
            .zip(l_distances)
            .zip(r_distances)
            .filter(|&((i, _), _)| i != self.arg_radial && i != arg_r)
            .partition::<Vec<_>, _>(|&((_, l), r)| l <= r);

        let (l_indices, r_indices) = {
            let mut l_indices = Self::drop_distances(l_indices);
            let mut r_indices = Self::drop_distances(r_indices);

            l_indices.push(self.arg_radial);
            r_indices.push(arg_r);

            (l_indices, r_indices)
        };

        if l_indices.len() < r_indices.len() {
            ([(arg_r, r_indices), (self.arg_radial, l_indices)], polar_distance)
        } else {
            ([(self.arg_radial, l_indices), (arg_r, r_indices)], polar_distance)
        }
    }

    /// Drops the distances from a vector, returning only the indices.
    fn drop_distances(indices: Vec<((usize, U), U)>) -> Vec<usize> {
        indices.into_iter().map(|((i, _), _)| i).collect()
    }

    /// The `history` of the child `Cluster`.
    ///
    /// # Arguments
    ///
    /// * `right`: Whether the child `Cluster` is the right child.
    fn child_history(&self, right: bool) -> Vec<bool> {
        let mut history = self.history.clone();
        history.push(right);
        history
    }

    /// Sets the `Cluster` ratios for anomaly detection and related applications.
    ///
    /// This method may only be called on the root `Cluster`. It is user error
    /// to call this method on a non-root `Cluster`.
    ///
    /// This method should be called after calling `partition` on the root
    /// `Cluster`. It is user error to call this method before calling
    /// `partition` on the root `Cluster`.
    ///
    /// # Arguments
    ///
    /// * `normalized`: Whether to normalize the ratios. We use Gaussian error
    /// functions to normalize the ratios, which is a common practice in
    /// anomaly detection.
    #[allow(unused_mut, unused_variables, dead_code)]
    pub fn with_ratios(mut self, normalized: bool) -> Self {
        todo!()
        // if !self.is_root() {
        //     panic!("This method may only be set from the root cluster.")
        // }
        // if self.is_leaf() {
        //     panic!("Please `build` and `partition` the tree before setting cluster ratios.")
        // }

        // match &self.index {
        //     Index::Indices(_) => panic!("Should not be here ..."),
        //     Index::Children(([(l, left), (r, right)], lr)) => {
        //         let left = Box::new(left.set_child_parent_ratios([1.; 6]));
        //         let right = Box::new(right.set_child_parent_ratios([1.; 6]));
        //         self.index = Index::Children(([(*l, left), (*r, right)], *lr));
        //     },
        // };
        // self.ratios = Some([1.; 6]);

        // if normalized {
        //     let ratios: Vec<_> = self.subtree().iter().flat_map(|c| c.ratios()).collect();
        //     let ratios: Vec<Vec<_>> = (0..6)
        //         .map(|s| ratios.iter().skip(s).step_by(6).cloned().collect())
        //         .collect();
        //     let means: [f64; 6] = ratios
        //         .iter()
        //         .map(|values| helpers::mean(values))
        //         .collect::<Vec<_>>()
        //         .try_into()
        //         .unwrap();
        //     let sds: [f64; 6] = ratios
        //         .iter()
        //         .zip(means.iter())
        //         .map(|(values, &mean)| 1e-8 + helpers::sd(values, mean))
        //         .collect::<Vec<_>>()
        //         .try_into()
        //         .unwrap();

        //     self.set_normalized_ratios(means, sds);
        // }

        // self
    }

    /// Sets the chile-parent `Cluster` ratios for anomaly detection and related
    /// applications.
    ///
    /// # Arguments
    ///
    /// * `parent_ratios`: The ratios for the parent `Cluster`.
    #[allow(unused_mut, unused_variables, dead_code)]
    fn set_child_parent_ratios(mut self, parent_ratios: Ratios) -> Self {
        todo!()
        // let [pc, pr, pl, pc_, pr_, pl_] = parent_ratios;

        // let c = (self.cardinality as f64) / pc;
        // let r = self.radius().as_f64() / pr;
        // let l = self.lfd() / pl;

        // let c_ = self.next_ema(c, pc_);
        // let r_ = self.next_ema(r, pr_);
        // let l_ = self.next_ema(l, pl_);

        // let ratios = [c, r, l, c_, r_, l_];
        // self.ratios = Some(ratios);

        // match &self.index {
        //     Index::Indices(_) => (),
        //     Index::Children(([(l, left), (r, right)], lr)) => {
        //         let left = Box::new(left.set_child_parent_ratios([1.; 6]));
        //         let right = Box::new(right.set_child_parent_ratios([1.; 6]));
        //         self.index = Index::Children(([(*l, left), (*r, right)], *lr));
        //     },
        // };

        // self
    }

    /// Normalizes the `Cluster` ratios for anomaly detection and related
    /// applications.
    ///
    /// # Arguments
    ///
    /// * `means`: The means of the `Cluster` ratios.
    /// * `sds`: The standard deviations of the `Cluster` ratios.
    #[allow(unused_mut, unused_variables, dead_code)]
    fn set_normalized_ratios(&mut self, means: Ratios, sds: Ratios) {
        todo!()
        // let ratios: Vec<_> = self
        //     .ratios
        //     .unwrap()
        //     .into_iter()
        //     .zip(means.into_iter())
        //     .zip(sds.into_iter())
        //     .map(|((value, mean), std)| (value - mean) / (std * 2_f64.sqrt()))
        //     .map(libm::erf)
        //     .map(|v| (1. + v) / 2.)
        //     .collect();
        // self.ratios = Some(ratios.try_into().unwrap());

        // match self.index {
        //     Index::Indices(_) => (),
        //     Index::Children(([(_, mut left), (_, mut right)], _)) => {
        //         left.set_normalized_ratios(means, sds);
        //         right.set_normalized_ratios(means, sds);
        //     },
        // };
    }

    /// The indices of the `Cluster`'s instances in the dataset.
    pub fn indices<'a, D: Dataset<T, U>>(&'a self, data: &'a D) -> &[usize] {
        &data.indices()[self.offset..(self.offset + self.cardinality)]
    }

    /// The `history` of the `Cluster` as a bool vector.
    ///
    /// The root `Cluster` has a `history` of length 1, being `vec![true]`. The
    /// history of a left child is the history of its parent with a `false`
    /// appended. The history of a right child is the history of its parent with
    /// a `true` appended.
    ///
    /// The `history` of a `Cluster` is used to identify the `Cluster` in the
    /// tree, and to compute the `Cluster`'s `name`.
    #[allow(dead_code)]
    pub fn history(&self) -> &[bool] {
        &self.history
    }
    /// The `name` of the `Cluster` as a hex-String.
    ///
    /// This is a human-readable representation of the `Cluster`'s `history`.
    /// It may be used to store the `Cluster` in a database, or to identify the
    /// `Cluster` in a visualization.
    pub fn name(&self) -> String {
        Self::history_to_name(&self.history)
    }

    /// Returns a human-readable hexidecimal representation of a `Cluster` history
    /// boolean vector
    #[allow(clippy::many_single_char_names)]
    pub fn history_to_name(history: &[bool]) -> String {
        let d = history.len();
        let padding = if d % 4 == 0 { 0 } else { 4 - d % 4 };
        let bin_name = (0..padding)
            .map(|_| "0")
            .chain(history.iter().map(|&b| if b { "1" } else { "0" }))
            .collect::<Vec<_>>();
        bin_name
            .chunks_exact(4)
            .map(|s| {
                let [a, b, c, d] = [s[0], s[1], s[2], s[3]];
                let s = format!("{a}{b}{c}{d}");
                let Ok(s) = u8::from_str_radix(&s, 2) else {
                    unreachable!("We know the characters used are only \"0\" and \"1\".")
                };
                format!("{s:01x}")
            })
            .collect()
    }

    /// Converts the hexidecimal representation of cluster history obtained from `Cluster::name`
    /// back into a `Vec<bool>`
    pub fn name_to_history(name: &str) -> Vec<bool> {
        let mut history = vec![];

        // Get the first 4 bits and dont add any 0 padding.
        // Unwrapping here is warranted because every name is at least one bit long and thus
        // the resulting string has nonzero length.
        #[allow(clippy::unwrap_used)]
        let mut chunk = name.chars().nth(0).unwrap() as u8;

        // If the chunk is 0-9, we subtract 48 to get its true value
        if chunk < 97 {
            chunk -= 48;
        }
        // Otherwise, it's in the a-f range, in which we subtract 97 to
        // zero it out and then add 10
        else {
            chunk -= 87;
        }

        // True iff. we've encountered a one bit in the chunk
        let mut found_high_bit = false;
        for i in 0..4 {
            // Get the 3 - i th high bit
            let mask = 1 << (3 - i);

            if chunk & mask > 0 {
                found_high_bit = true;
            }

            if found_high_bit {
                history.push((chunk & mask) > 0);
            }
        }

        for c in (name[1..]).chars() {
            // Each char gets converted into 4 bits
            let mut chunk = c as u8;

            // If the chunk is 0-9, we subtract 48 to get its true value
            if chunk < 97 {
                chunk -= 48;
            }
            // Otherwise, it's in the a-f range, in which we subtract 97 to
            // zero it out and then add 10
            else {
                chunk -= 87;
            }

            for i in 0..4 {
                // Get the 3 - i th high bit
                let mask = 1 << (3 - i);
                history.push((chunk & mask) > 0);
            }
        }
        history
    }
    /// Whether the `Cluster` is the root of the tree.
    ///
    /// The root `Cluster` has a depth of 0.
    #[allow(dead_code)]
    pub fn is_root(&self) -> bool {
        self.depth() == 0
    }

    /// The depth of the `Cluster` in the tree.
    ///
    /// The root `Cluster` has a depth of 0. The depth of a child is the depth
    /// of its parent plus 1.
    pub fn depth(&self) -> usize {
        self.history.len() - 1
    }

    /// Whether the `Cluster` contains only one instance or only identical
    /// instances.
    pub fn is_singleton(&self) -> bool {
        self.radius == U::zero()
    }

    /// Whether this cluster has no children.
    pub const fn is_leaf(&self) -> bool {
        self.children.is_none()
    }

    /// A 2-slice of references to the left and right child `Cluster`s.
    pub fn children(&self) -> Option<[&Self; 2]> {
        self.children.as_ref().map(|v| [v.left.as_ref(), v.right.as_ref()])
    }

    /// The distance between the poles of the `Cluster`.
    #[allow(dead_code)]
    pub fn polar_distance(&self) -> Option<U> {
        self.children.as_ref().map(|v| v.polar_distance)
    }

    /// The six `Cluster` ratios used for anomaly detection and related
    /// applications.
    ///
    /// These ratios are:
    ///
    /// * child-cardinality / parent-cardinality.
    /// * child-radius / parent-radius.
    /// * child-lfd / parent-lfd.
    /// * EMA of child-cardinality / parent-cardinality.
    /// * EMA of child-radius / parent-radius.
    /// * EMA of child-lfd / parent-lfd.
    ///
    /// This method may only be called after calling `with_ratios` on the root.
    /// It is user error to call this method before calling `with_ratios` on
    /// the root.
    #[allow(dead_code)]
    pub fn ratios(&self) -> Ratios {
        self.ratios.unwrap_or([0.; 6])
    }

    /// Whether this `Cluster` is an ancestor of the `other` `Cluster`.
    #[allow(dead_code)]
    pub fn is_ancestor_of(&self, other: &Self) -> bool {
        self.depth() < other.depth() && self.history.as_slice() == &other.history[..self.history.len()]
    }

    /// Whether this `Cluster` is an descendant of the `other` `Cluster`.
    #[allow(dead_code)]
    pub fn is_descendant_of(&self, other: &Self) -> bool {
        other.is_ancestor_of(self)
    }

    /// A Vec of references to all `Cluster`s in the subtree of this `Cluster`,
    /// including this `Cluster`.
    pub fn subtree(&self) -> Vec<&Self> {
        let subtree = vec![self];

        // Two scenarios: Either we have children or not
        match self.children() {
            Some([left, right]) => subtree
                .into_iter()
                .chain(left.subtree())
                .chain(right.subtree())
                .collect(),

            None => subtree,
        }
    }

    /// The maximum depth of any leaf in the subtree of this `Cluster`.
    pub fn max_leaf_depth(&self) -> usize {
        self.subtree().into_iter().map(Self::depth).max().map_or_else(
            || unreachable!("The subtree of a Cluster should have at least one element, i.e. the Cluster itself."),
            |depth| depth,
        )
    }

    /// Distance from the `center` to the given instance.
    pub fn distance_to_instance<D: Dataset<T, U>>(&self, data: &D, instance: T) -> U {
        data.query_to_one(instance, self.arg_center)
    }

    /// Distance from the `center` of this `Cluster` to the center of the
    /// `other` `Cluster`.
    #[allow(dead_code)]
    pub fn distance_to_other<D: Dataset<T, U>>(&self, data: &D, other: &Self) -> U {
        data.one_to_one(self.arg_center, other.arg_center)
    }

    /// Assuming that this `Cluster` overlaps with with query ball, we return
    /// only those children that also overlap with the query ball
    pub fn overlapping_children<D: Dataset<T, U>>(&self, data: &D, query: T, radius: U) -> Vec<&Self> {
        self.children.as_ref().map_or_else(
            Vec::new,
            |Children {
                 left,
                 right,
                 arg_l,
                 arg_r,
                 polar_distance,
                 ..
             }| {
                let ql = data.query_to_one(query, *arg_l);
                let qr = data.query_to_one(query, *arg_r);

                let swap = ql < qr;
                let (ql, qr) = if swap { (qr, ql) } else { (ql, qr) };

                if (ql + qr) * (ql - qr) <= U::from(2) * (*polar_distance) * radius {
                    vec![left.as_ref(), right.as_ref()]
                } else if swap {
                    vec![left.as_ref()]
                } else {
                    vec![right.as_ref()]
                }
            },
        )
    }
}

use serde::{Deserialize, Serialize};

/// Serialized information about a given `Cluster`'s children
#[derive(Serialize, Deserialize)]
pub struct SerializedChildInfo {
    /// The encoded history of the left child
    pub left_name: String,
    /// The encoded history of the right child
    pub right_name: String,
    /// The left pole of the `Cluster`
    pub arg_l: usize,
    /// The right pole of the `Cluster`
    pub arg_r: usize,
    /// The distance from the `l_pole` to the `r_pole` in bytes.
    /// This value gets reconstituted can be reconstituted from
    /// whatever `U: Number` it was decomposed from
    pub polar_distance_bytes: Vec<u8>,
}

/// Intermediate representation of `Cluster` for serialization.
/// We do this instead of directly serializing/deserializing the Clusters themselves because
/// writing a deserializer directly is moderately complicated with our exceptions
#[allow(clippy::module_name_repetitions)]
#[derive(Serialize, Deserialize)]
pub struct SerializedCluster {
    /// The encoded history of the `Cluster`
    pub name: String,
    /// The seed (if applicable) that the `Cluster` was constructed with
    pub seed: Option<u64>,
    /// The `Cluster`'s offset
    pub offset: usize,
    /// The `Cluster`'s cardinality
    pub cardinality: usize,
    /// The `Cluster`'s arg_center
    pub arg_center: usize,
    /// The `Cluster`'s arg_radial
    pub arg_radial: usize,
    /// The `Cluster`'s radius in byte form
    pub radius_bytes: Vec<u8>,
    /// The `Cluster`'s local fractal dimension
    pub lfd: f64,
    /// The `Cluster`'s ratios
    pub ratios: Option<Ratios>,
    /// Serialized information about the cluster's immediate children, if applicable
    pub child_info: Option<SerializedChildInfo>,
}

impl SerializedCluster {
    /// Converts a `Cluster` to a `SerializedCluster`
    #[allow(dead_code)]
    pub fn from_cluster<T: Send + Sync + Copy, U: Number>(cluster: &Cluster<T, U>) -> Self {
        let name = cluster.name();
        let cardinality = cluster.cardinality;
        let offset = cluster.offset;
        let seed = cluster.seed;

        // Because Number isn't serializeable, we just convert it to bytes and
        // serialize that
        let radius_bytes = cluster.radius.to_le_bytes();
        let arg_center = cluster.arg_center;
        let arg_radial = cluster.arg_radial;
        let lfd = cluster.lfd;

        let ratios = cluster.ratios;

        // Since we cant do this recursively we need to like depth-first traverse
        // the tree and serialize manually
        #[allow(clippy::option_if_let_else)]
        let child_info = {
            if let Some(children) = &cluster.children {
                let Children {
                    left,
                    right,
                    arg_l,
                    arg_r,
                    polar_distance,
                    _t: _,
                } = children;

                Some(SerializedChildInfo {
                    left_name: left.name(),
                    right_name: right.name(),
                    arg_l: *arg_l,
                    arg_r: *arg_r,
                    polar_distance_bytes: polar_distance.to_le_bytes(),
                })
            } else {
                None
            }
        };

        Self {
            name,
            seed,
            offset,
            cardinality,
            arg_center,
            arg_radial,
            radius_bytes,
            lfd,
            ratios,
            child_info,
        }
    }

    #[allow(dead_code)]
    /// Converts a `SerializedCluster` to a `Cluster`. Optionally returns information about the
    /// Children's poles
    pub fn into_partial_cluster<T: Send + Sync + Copy, U: Number>(self) -> (Cluster<T, U>, Option<SerializedChildInfo>) {
        (
            Cluster {
                history: Cluster::<T, U>::name_to_history(&self.name),
                seed: self.seed,
                offset: self.offset,
                cardinality: self.cardinality,
                arg_center: self.arg_center,
                arg_radial: self.arg_radial,
                radius: U::from_le_bytes(&self.radius_bytes),
                lfd: self.lfd,
                ratios: self.ratios,
                children: None,
            },
            self.child_info,
        )
    }
}

#[cfg(test)]
mod tests {
    use distances::vectors::euclidean;

    use crate::{
        Tree, {Dataset, VecDataset},
    };

    use super::*;

    #[test]
    fn test_cluster() {
        let data: Vec<&[f32]> = vec![&[0., 0., 0.], &[1., 1., 1.], &[2., 2., 2.], &[3., 3., 3.]];
        let name = "test".to_string();
        let mut data = VecDataset::new(name, data, euclidean::<f32, f32>, false);
        let indices = data.indices().to_vec();
        let partition_criteria = PartitionCriteria::new(true).with_max_depth(3).with_min_cardinality(1);
        let root = Cluster::new_root(&data, &indices, Some(42)).partition(&mut data, &partition_criteria);

        assert!(!root.is_leaf());
        assert!(root.children().is_some());

        assert_eq!(root.depth(), 0);
        assert_eq!(root.cardinality, 4);
        assert_eq!(root.subtree().len(), 7);
        assert!(root.radius > 0.);

        assert_eq!(format!("{root}"), "1");

        let Some([left, right]) = root.children() else {
            unreachable!("The root cluster has children.")
        };
        assert_eq!(format!("{left}"), "2");
        assert_eq!(format!("{right}"), "3");

        for child in [left, right] {
            assert_eq!(child.depth(), 1);
            assert_eq!(child.cardinality, 2);
            assert_eq!(child.subtree().len(), 3);
        }

        let subtree = root.subtree();
        assert_eq!(
            subtree.len(),
            7,
            "The subtree of the root cluster should have 7 elements but had {}.",
            subtree.len()
        );
        for c in root.subtree() {
            let radius = data.one_to_one(c.arg_center, c.arg_radial);
            assert!(
                (radius - c.radius).abs() <= f32::EPSILON,
                "Radius must be equal to the distance to the farthest instance. {c} had radius {} but distance {radius}.",
                c.radius,
            );
        }
    }

    #[test]
    fn test_leaf_indices() {
        let data: Vec<&[f32]> = vec![&[10.], &[1.], &[-5.], &[8.], &[3.], &[2.], &[0.5], &[0.]];
        let name = "test".to_string();
        let data = VecDataset::new(name, data, euclidean::<f32, f32>, false);
        let partition_criteria = PartitionCriteria::new(true).with_max_depth(3).with_min_cardinality(1);

        let tree = Tree::new(data, Some(42)).partition(&partition_criteria);

        let mut leaf_indices = tree.root.indices(tree.data()).to_vec();
        leaf_indices.sort_unstable();

        assert_eq!(leaf_indices, tree.data().indices());
    }

    #[test]
    fn test_end_to_end_reordering() {
        let data: Vec<&[f32]> = vec![&[10.], &[1.], &[-5.], &[8.], &[3.], &[2.], &[0.5], &[0.]];
        let name = "test".to_string();
        let data = VecDataset::new(name, data, euclidean::<f32, f32>, false);
        let partition_criteria = PartitionCriteria::new(true).with_max_depth(3).with_min_cardinality(1);

        let tree = Tree::new(data, Some(42)).partition(&partition_criteria);

        // Assert that the root's indices actually cover the whole dataset.
        assert_eq!(tree.data().cardinality(), tree.indices().len());

        // Assert that the tree's indices have been reordered in depth-first order
        assert_eq!((0..tree.cardinality()).collect::<Vec<usize>>(), tree.indices());
    }

    #[test]
    fn cluster() {
        let (dimensionality, min_val, max_val) = (10, -1., 1.);
        let seed = 42;

        let data = symagen::random_data::random_f32(10_000, dimensionality, min_val, max_val, seed);
        let data = data.iter().map(Vec::as_slice).collect::<Vec<_>>();
        let name = "test".to_string();
        let mut data = VecDataset::<_, f32>::new(name, data, euclidean, false);
        let indices = data.indices().to_vec();
        let partition_criteria = PartitionCriteria::new(true).with_min_cardinality(1);

        let root = Cluster::new_root(&data, &indices, Some(seed)).partition(&mut data, &partition_criteria);

        for c in root.subtree() {
            assert!(c.cardinality > 0, "Cardinality must be positive.");
            assert!(c.radius >= 0., "Radius must be non-negative.");
            assert!(c.lfd > 0., "LFD must be positive.");

            let radius = data.one_to_one(c.arg_center, c.arg_radial);
            assert!(
                (radius - c.radius).abs() <= f32::EPSILON,
                "Radius must be equal to the distance to the farthest instance. {c} had radius {} but distance {radius}.",
                c.radius,
            );
        }
    }

    mod serialize_tests {
        use super::*;
        use crate::{Dataset, VecDataset};
        use distances::vectors::euclidean;
        use rand::Rng;

        #[test]
        fn test_basic() {
            let data: Vec<&[f32]> = vec![&[0., 0., 0.], &[1., 1., 1.], &[2., 2., 2.], &[3., 3., 3.]];
            let name = "test".to_string();
            let data = VecDataset::new(name, data, euclidean::<f32, f32>, false);
            let indices = data.indices().to_vec();
            let mut c1 = Cluster::new_root(&data, &indices, Some(42));
            c1.history = vec![true, true, false, false, true];

            let s1 = SerializedCluster::from_cluster(&c1);
            let s1_string = serde_json::to_string(&s1).unwrap();

            let s2: SerializedCluster = serde_json::from_str(&s1_string).unwrap();
            assert_eq!(s1.name, s2.name);
            assert_eq!(s1.seed, s2.seed);
            assert_eq!(s1.offset, s2.offset);
            assert_eq!(s1.cardinality, s2.cardinality);
            assert_eq!(s1.arg_center, s2.arg_center);
            assert_eq!(s1.arg_radial, s2.arg_radial);
            assert_eq!(s1.radius_bytes, s2.radius_bytes);
            assert_eq!(s1.lfd, s2.lfd);
            assert_eq!(s1.ratios, s2.ratios);

            let (c2, chls) = s2.into_partial_cluster();

            assert_eq!(c1, c2);
            assert!(chls.is_none());
        }

        #[test]
        fn test_history_to_name() {
            use rand::distributions::{Bernoulli, Distribution};

            let d = Bernoulli::new(0.3).unwrap();

            for length in 1..800 {
                let mut hist = vec![true];

                for _ in 1..length {
                    let b = d.sample(&mut rand::thread_rng());
                    hist.push(b);
                }

                let name = Cluster::<&[f32], f32>::history_to_name(&hist);
                let recovered = Cluster::<&[f32], f32>::name_to_history(&name);
                assert_eq!(recovered, hist);
            }
        }

        #[test]
        fn test_name_to_history() {
            let charset = "0123456789abcdef";
            let mut rng = rand::thread_rng();

            for length in 1..200 {
                // Randomly choose the first char. Must be nonzero
                let idx = rng.gen_range(1..charset.len());
                let c = charset.chars().nth(idx).unwrap();
                let mut name = String::from(c);

                // Randomly choose the remaining characters
                for _ in 1..length {
                    let idx = rng.gen_range(0..charset.len());
                    let c = charset.chars().nth(idx).unwrap();
                    name.push(c);
                }

                let hist = Cluster::<&[f32], f32>::name_to_history(&name);
                let recovered_name = Cluster::<&[f32], f32>::history_to_name(&hist);

                assert_eq!(recovered_name, name);
            }
        }
    }
}
