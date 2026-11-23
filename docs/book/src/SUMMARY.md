# Summary

[Introduction](introduction.md)
[Conventions](conventions.md)

# Part I: Foundations

- [Your first plan](part-1/01-hello-plan.md)
- [Configuration](part-1/02-configuration.md)
- [The context](part-1/03-context-and-session.md)
- [Types, schemas and expressions](part-1/04-schemas-and-expressions.md)
- [Relations and plans](part-1/05-relations-and-plans.md)
- [Prepare and execute](part-1/06-prepare-and-execute.md)
- [Aggregates, joins and ordering](part-1/07-aggregates-joins-and-ordering.md)

# Part II: The explicit plan

- [Everything in the plan](part-2/01-everything-in-the-plan.md)
- [Workers and distribution](part-2/02-workers-and-distribution.md)
- [Exchanges](part-2/03-exchanges.md)
- [Stages and bindings](part-2/04-stages-and-bindings.md)
- [Runtime filters](part-2/05-runtime-filters.md)
- [Adaptive plans](part-2/06-adaptive-plans.md)
- [Validation](part-2/07-validation.md)

# Part III: Query walkthroughs

- [The running example](part-3/00-running-example.md)
- [Scan, filter and project](part-3/01-scan-filter-project.md)
- [Aggregation](part-3/02-aggregation.md)
- [Broadcast join](part-3/03-broadcast-join.md)
- [Partitioned join and runtime filters](part-3/04-partitioned-join-and-runtime-filters.md)
- [Adaptive join](part-3/05-adaptive-join.md)
- [Sort, top-N and limit](part-3/06-sort-top-n-and-limit.md)
- [A multi-stage query](part-3/07-a-multi-stage-query.md)
- [Live inputs and union]()

# Part IV: Ways to use Sirius

- [Embedded in a host: sirius-duckdb](part-4/01-duckdb-extension.md)
- [Under someone else's scheduler: StarRocks](part-4/02-starrocks.md)
- [Sirius on its own: Distributed Sirius](part-4/03-distributed-sirius.md)
- [One IR, three integrations](part-4/04-one-ir-three-integrations.md)
- [Plans from other systems: Velox and Substrait](part-4/05-plans-from-other-systems.md)

# Part V: Inside the engine

- [From plan to pipelines](part-5/01-from-plan-to-pipelines.md)
- [Exchanges in the engine](part-5/02-exchanges-in-the-engine.md)
- [Runtime values in the engine](part-5/03-runtime-values-in-the-engine.md)
- [Adaptivity in the engine](part-5/04-adaptivity-in-the-engine.md)
- [Plan-to-plan rewrites](part-5/05-plan-to-plan-rewrites.md)
- [Memory](part-5/06-memory.md)
- [Failure, cancellation and recovery](part-5/07-failure-cancellation-and-recovery.md)
- [Running at scale](part-5/08-running-at-scale.md)

# Appendix

- [Related designs](appendix/related-designs.md)

# Reference

- [Node catalog](reference/node-catalog.md)
- [API at a glance](reference/api-at-a-glance.md)
- [Plan notation](reference/plan-notation.md)
- [Glossary](reference/glossary.md)
- [Open questions](reference/open-questions.md)
- [Extension points](reference/extension-points.md)
- [The C ABI]()
- [Arrow interoperability]()
- [Streaming inputs and buffering]()
