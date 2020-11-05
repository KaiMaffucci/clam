""" This module provides the CHAODA algorithms implemented on top of CLAM.
"""

from typing import List, Dict, Set

import numpy as np
from scipy.special import erf

from pyclam import Graph, Cluster, Edge


def _catch_normalization_mode(mode: str) -> None:
    modes: List[str] = ['linear', 'gaussian', 'sigmoid']
    if mode not in modes:
        raise ValueError(f'Normalization method {mode} is undefined. Must by one of {modes}.')
    else:
        return


def _normalize(scores: np.array, mode: str) -> np.array:
    if mode == 'linear':
        min_v, max_v, = float(np.min(scores)), float(np.max(scores))
        if min_v == max_v:
            max_v += 1.
        scores = (scores - min_v) / (max_v - min_v)
    else:
        mu: float = float(np.mean(scores))
        sigma: float = max(float(np.std(scores)), 1e-3)

        if mode == 'gaussian':
            scores = erf((scores - mu) / (sigma * np.sqrt(2)))
        else:
            scores = 1 / (1 + np.exp(-(scores - mu) / sigma))

    return scores.ravel().clip(0, 1)


def cluster_cardinality(graph: Graph, normalize: str = 'gaussian') -> np.array:
    """ Determines outlier scores for points by considering the relative cardinalities of the clusters in the graph.

    Points in clusters with relatively low cardinalities are the outliers.

    :param graph: Graph on which to calculate outlier scores.
    :param normalize: which normalization mode to use to get scores in a [0, 1] range.
                      Must be one of 'linear', 'gaussian', or 'sigmoid'.
    :return: A numpy array of outlier scores for each point in the manifold that the graph belongs to.
    """
    _catch_normalization_mode(normalize)

    scores: Dict[int, int] = {p: -cluster.cardinality for cluster in graph.clusters for p in cluster.argpoints}
    scores: List[int] = [scores[i] for i in range(len(scores))]
    return _normalize(np.asarray(scores, dtype=float), normalize)


def parent_child(graph: Graph, normalize: str = 'gaussian') -> np.array:
    """ Determines outlier scores for points by considering ratios of cardinalities of parent-child clusters.

    The ratios are weighted by the child's depth in the tree, and are then accumulated for each point in each cluster in the graph.
    Points with relatively low accumulated ration are the outliers.

    :param graph: Graph on which to calculate outlier scores.
    :param normalize: which normalization mode to use to get scores in a [0, 1] range.
                      Must be one of 'linear', 'gaussian', or 'sigmoid'.
    :return: A numpy array of outlier scores for each point in the manifold that the graph belongs to.
    """
    _catch_normalization_mode(normalize)

    results: np.array = np.zeros(shape=graph.manifold.data.shape[0], dtype=float)
    for cluster in graph:
        ancestry = graph.manifold.ancestry(cluster)
        for i in range(1, len(ancestry)):
            score = float(ancestry[i-1].cardinality) / (ancestry[i].cardinality * np.sqrt(i))
            for p in cluster.argpoints:
                results[p] += score
    return _normalize(results, normalize)


def graph_neighborhood(graph: Graph, normalize: str = 'gaussian') -> np.array:
    """ Determines outlier scores by the considering the relative graph-neighborhood of clusters.

    Points in clusters with relatively small neighborhoods are the outliers.

    :param graph: Graph on which to calculate outlier scores.
    :param normalize: which normalization mode to use to get scores in a [0, 1] range.
               Must be one of 'linear', 'gaussian', or 'sigmoid'.

    :return: A numpy array of outlier scores for each point in the manifold that the graph belongs to.
    """
    _catch_normalization_mode(normalize)

    def _bft(start: Cluster) -> List[int]:
        path_length: List[int] = list()
        visited: Set[Cluster] = set()
        frontier: Set[Cluster] = {start}
        while frontier:
            visited += frontier
            path_length.append(len(visited))
            frontier = {neighbor for cluster in frontier for neighbor in graph.neighbors(cluster)}
            frontier -= visited
        return path_length

    path_lengths: Dict[Cluster, List[int]] = {cluster: _bft(cluster) for cluster in graph.clusters}
    scores: Dict[Cluster, int] = {cluster: path_length[len(path_lengths) // 4] if len(path_lengths) > 0 else 0 for cluster, path_length in path_lengths.items()}
    scores: Dict[int, int] = {point: -score for cluster, score in scores.items() for point in cluster.argpoints}
    return _normalize(np.asarray([scores[i] for i in range(len(scores))], dtype=float), normalize)


def component_cardinality(graph: Graph, normalize: str = 'gaussian') -> np.array:
    """ Determines outlier scores by considering the relative cardinalities of the connected components of the graph.

    Points in components of relatively low cardinalities are the outliers

    :param graph: Graph on which to calculate outlier scores.
    :param normalize: which normalization mode to use to get scores in a [0, 1] range.
                      Must be one of 'linear', 'gaussian', or 'sigmoid'.
    :return: A numpy array of outlier scores for each point in the manifold that the graph belongs to.
    """
    _catch_normalization_mode(normalize)

    scores: Dict[int, int] = {
        p: -component.cardinality
        for component in graph.components
        for cluster in component.clusters
        for p in cluster.argpoints
    }
    return _normalize(np.asarray([scores[i] for i in range(len(scores))], dtype=float), normalize)


def _compute_transition_probabilities(graph: Graph, cluster: Cluster) -> Dict[Edge, float]:
    """ returns the outgoing transition probabilities from the given cluster. """
    if len(graph.edges_from(cluster)) > 0:
        factor: float = sum([edge.distance for edge in graph.edges_from(cluster)])
        probabilities: Dict[Edge, float] = {edge: edge.distance / factor for edge in graph.edges_from(cluster)}

        # TODO: if this never breaks in testing, remove the assert
        sum_probabilities: float = sum(probabilities.values()) - 1.
        assert abs(sum_probabilities) < 1e-3, f'probabilities did not sum to 1. sum: {sum_probabilities + 1:.3f},\n' \
                                              f'values: {[str(round(p, 4)) for p in probabilities.values()]}'
        return probabilities
    else:
        return dict()


def _perform_random_walks(
        component: Graph,
        starts: Set[Cluster],
        steps_multiplier: int,
        transition_probabilities: Dict[Cluster, Dict[Edge, float]],
) -> Dict[Cluster, int]:
    """ performs random walks on one connected component of a graph. """
    if component.cardinality > 1:
        visit_counts: Dict[Cluster, int] = {cluster: 1 if cluster in starts else 0 for cluster in component.clusters}
        locations: List[Cluster] = list(starts)
        for _ in range(component.cardinality * steps_multiplier):
            locations = [np.random.choice(  # randomly choose a neighbor to move to from each location
                a=list(transition_probabilities[cluster].keys()),
                p=list(transition_probabilities[cluster].values()),
            ) for cluster in locations]
            visit_counts.update({cluster: visit_counts[cluster] + 1 for cluster in locations})
        return visit_counts
    else:
        return {next(iter(component.clusters)): 1}


def random_walk(graph: Graph, normalize: str = 'gaussian') -> np.array:
    """ Determines outlier scores by performing random walks on the graph.

    Points that are visited less often, relatively, are the outliers.

    :param graph: Graph on which to calculate outlier scores.
    :param normalize: which normalization mode to use to get scores in a [0, 1] range.
                       Must be one of 'linear', 'gaussian', or 'sigmoid'.
    :return: A numpy array of outlier scores for each point in the manifold that the graph belongs to.
    """
    _catch_normalization_mode(normalize)

    # determine subsumed and walkable clusters
    subsumed_clusters: Set[Cluster] = set()
    for edge in graph.edges:
        if edge.distance + edge.left.radius < edge.right.radius:
            subsumed_clusters.add(edge.left)
        elif edge.distance + edge.right.radius < edge.left.radius:
            subsumed_clusters.add(edge.right)

    walkable_clusters: Set[Cluster] = {cluster for cluster in graph.clusters if cluster not in subsumed_clusters}
    walkable_graph: Graph = graph.subgraph(walkable_clusters)

    # compute transition probabilities
    transition_probabilities: Dict[Cluster, Dict[Edge, float]] = {
        cluster: _compute_transition_probabilities(graph, cluster)
        for cluster in walkable_clusters
    }

    # perform walks on each subgraph of walkable graph
    visit_counts: Dict[Cluster, int] = dict()
    for component in walkable_graph.components:
        starts: Set[Cluster] = component.clusters if component.cardinality < 100 else set(list(component.clusters)[:100])
        visit_counts.update(_perform_random_walks(component, starts, 10, transition_probabilities))

    # create dict of walkable cluster -> set of subsumed clusters
    subsumed_neighbors: Dict[Cluster, Set[Cluster]] = {cluster: set() for cluster in walkable_clusters}
    for edge in graph.edges:
        if set(edge.clusters).issubset(walkable_clusters):  # both clusters are walkable
            continue
        elif edge.left in walkable_clusters:  # only left is walkable
            if edge.right not in walkable_clusters:  # right is subsumed
                subsumed_neighbors[edge.left].add(edge.right)
        elif edge.right in walkable_clusters:  # only right is walkable
            if edge.left not in walkable_clusters:  # left is subsumed
                subsumed_neighbors[edge.right].add(edge.left)
        else:  # neither is walkable
            # if both are subsumed, it gets accounted for with a different edge
            continue

    # update visit-counts for subsumed clusters
    for master, subsumed in subsumed_neighbors.items():
        for cluster in subsumed:
            if cluster in visit_counts:
                visit_counts[cluster] += visit_counts[master]
            else:
                visit_counts[cluster] = visit_counts[master]

    # normalize counts
    scores: Dict[int, int] = {point: -count for cluster, count in visit_counts.items() for point in cluster.argpoints}
    return _normalize(np.array([scores[i] for i in range(graph.cardinality)], dtype=int), mode=normalize)


# TODO: Add ensemble