//! Repository trait and SeaORM-backed implementation for branches.

use std::collections::HashMap;

use async_trait::async_trait;
use sea_orm::{
    sea_query::{LikeExpr, Query, SelectStatement},
    ActiveValue::{NotSet, Set},
    ColumnTrait, DatabaseConnection, DatabaseTransaction, DbBackend, EntityTrait, FromQueryResult,
    QueryFilter, QueryOrder, QuerySelect, Statement, TransactionTrait,
};
use unclip_core::{parent_of, Branch, Reference, SampleQuery};
use unclip_entity::{branch_o2m_values, branch_o2o_values, branch_references, branches};

use crate::mapper;

/// A distinct indexed value with how many branches carry it. Used to build
/// o2o/o2m catalogs (`unclip o2o`, `unclip o2m`).
#[derive(Debug, Clone, PartialEq, Eq, FromQueryResult)]
pub struct IndexedValue {
    pub name: String,
    pub value: String,
    pub count: i64,
}

/// Persistence boundary for branches. Application logic depends on this trait,
/// not on SeaORM entities directly (DRAFT §24).
#[async_trait]
pub trait BranchRepository {
    async fn add(&self, branch: Branch) -> anyhow::Result<()>;
    async fn get(&self, path: &str) -> anyhow::Result<Option<Branch>>;
    async fn update(&self, branch: Branch) -> anyhow::Result<()>;
    async fn delete(&self, path: &str) -> anyhow::Result<()>;

    async fn children(&self, path: &str) -> anyhow::Result<Vec<Branch>>;
    async fn descendants(&self, path: &str) -> anyhow::Result<Vec<Branch>>;
    async fn ancestors(&self, path: &str) -> anyhow::Result<Vec<Branch>>;

    async fn find(&self, query: SampleQuery) -> anyhow::Result<Vec<Branch>>;

    /// Insert or replace many branches (upsert by path), returning the
    /// `(added, updated)` counts.
    ///
    /// Implementations should run the whole batch atomically. The default
    /// falls back to a per-branch, non-transactional upsert.
    async fn upsert_many(&self, branches: Vec<Branch>) -> anyhow::Result<(usize, usize)> {
        let (mut added, mut updated) = (0usize, 0usize);
        for mut branch in branches {
            branch.id = None;
            if self.get(&branch.path).await?.is_some() {
                self.update(branch).await?;
                updated += 1;
            } else {
                self.add(branch).await?;
                added += 1;
            }
        }
        Ok((added, updated))
    }
}

/// SeaORM implementation backed by SQLite.
pub struct SeaOrmBranchRepository {
    db: DatabaseConnection,
}

impl SeaOrmBranchRepository {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }

    pub fn connection(&self) -> &DatabaseConnection {
        &self.db
    }

    /// Insert a branch's child rows (o2o/o2m/references) within a transaction.
    async fn insert_children(
        txn: &DatabaseTransaction,
        branch_id: i32,
        branch: &Branch,
    ) -> anyhow::Result<()> {
        let o2o = mapper::o2o_active_models(branch_id, branch);
        if !o2o.is_empty() {
            branch_o2o_values::Entity::insert_many(o2o).exec(txn).await?;
        }
        let o2m = mapper::o2m_active_models(branch_id, branch);
        if !o2m.is_empty() {
            branch_o2m_values::Entity::insert_many(o2m).exec(txn).await?;
        }
        let refs = mapper::reference_active_models(branch_id, branch);
        if !refs.is_empty() {
            branch_references::Entity::insert_many(refs).exec(txn).await?;
        }
        Ok(())
    }

    /// Replace a branch's child rows wholesale (delete then re-insert) within a
    /// transaction.
    async fn replace_children(
        txn: &DatabaseTransaction,
        branch_id: i32,
        branch: &Branch,
    ) -> anyhow::Result<()> {
        branch_o2o_values::Entity::delete_many()
            .filter(branch_o2o_values::Column::BranchId.eq(branch_id))
            .exec(txn)
            .await?;
        branch_o2m_values::Entity::delete_many()
            .filter(branch_o2m_values::Column::BranchId.eq(branch_id))
            .exec(txn)
            .await?;
        branch_references::Entity::delete_many()
            .filter(branch_references::Column::BranchId.eq(branch_id))
            .exec(txn)
            .await?;
        Self::insert_children(txn, branch_id, branch).await
    }

    /// Load a branch's child rows and assemble the full domain value.
    ///
    /// Delegates to `hydrate_all` so single- and multi-branch loads share one
    /// grouping/assembly path.
    async fn hydrate(&self, model: branches::Model) -> anyhow::Result<Branch> {
        Ok(self
            .hydrate_all(vec![model])
            .await?
            .pop()
            .expect("hydrate_all yields one branch for one model"))
    }

    /// Hydrate many branches with a fixed number of queries (no N+1): load all
    /// child rows for the whole id set at once, then group them per branch.
    async fn hydrate_all(&self, models: Vec<branches::Model>) -> anyhow::Result<Vec<Branch>> {
        if models.is_empty() {
            return Ok(Vec::new());
        }
        let ids: Vec<i32> = models.iter().map(|m| m.id).collect();

        let o2o = branch_o2o_values::Entity::find()
            .filter(branch_o2o_values::Column::BranchId.is_in(ids.clone()))
            .all(&self.db)
            .await?;
        let o2m = branch_o2m_values::Entity::find()
            .filter(branch_o2m_values::Column::BranchId.is_in(ids.clone()))
            .all(&self.db)
            .await?;
        let refs = branch_references::Entity::find()
            .filter(branch_references::Column::BranchId.is_in(ids))
            .order_by_asc(branch_references::Column::Id)
            .all(&self.db)
            .await?;

        let mut o2o_by_id: HashMap<i32, Vec<branch_o2o_values::Model>> = HashMap::new();
        for row in o2o {
            o2o_by_id.entry(row.branch_id).or_default().push(row);
        }
        let mut o2m_by_id: HashMap<i32, Vec<branch_o2m_values::Model>> = HashMap::new();
        for row in o2m {
            o2m_by_id.entry(row.branch_id).or_default().push(row);
        }
        let mut refs_by_id: HashMap<i32, Vec<branch_references::Model>> = HashMap::new();
        for row in refs {
            refs_by_id.entry(row.branch_id).or_default().push(row);
        }

        let mut out = Vec::with_capacity(models.len());
        for model in models {
            let id = model.id;
            out.push(mapper::assemble_branch(
                model,
                o2o_by_id.remove(&id).unwrap_or_default(),
                o2m_by_id.remove(&id).unwrap_or_default(),
                refs_by_id.remove(&id).unwrap_or_default(),
            )?);
        }
        Ok(out)
    }

    async fn model_by_path(&self, path: &str) -> anyhow::Result<Option<branches::Model>> {
        Ok(branches::Entity::find()
            .filter(branches::Column::Path.eq(path))
            .one(&self.db)
            .await?)
    }

    async fn load_branches_by_ids(&self, ids: Vec<i32>) -> anyhow::Result<Vec<Branch>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let models = branches::Entity::find()
            .filter(branches::Column::Id.is_in(ids))
            .all(&self.db)
            .await?;
        self.hydrate_all(models).await
    }

    /// Distinct o2o `name=value` pairs with branch counts, optionally for a
    /// single name. Ordered by name then value.
    pub async fn o2o_catalog(&self, name: Option<&str>) -> anyhow::Result<Vec<IndexedValue>> {
        self.index_catalog("branch_o2o_values", name).await
    }

    /// Distinct o2m `name=value` pairs with branch counts, optionally for a
    /// single name. Ordered by name then value.
    pub async fn o2m_catalog(&self, name: Option<&str>) -> anyhow::Result<Vec<IndexedValue>> {
        self.index_catalog("branch_o2m_values", name).await
    }

    async fn index_catalog(
        &self,
        table: &str,
        name: Option<&str>,
    ) -> anyhow::Result<Vec<IndexedValue>> {
        // `table` is a fixed internal identifier, never user input.
        let (filter, values) = match name {
            Some(n) => ("WHERE name = ?", vec![n.into()]),
            None => ("", Vec::new()),
        };
        let sql = format!(
            "SELECT name, value, COUNT(*) AS count FROM {table} {filter} \
             GROUP BY name, value ORDER BY name, value"
        );
        let rows = IndexedValue::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            sql,
            values,
        ))
        .all(&self.db)
        .await?;
        Ok(rows)
    }

    /// Branches carrying a specific o2o value.
    pub async fn branches_with_o2o(&self, name: &str, value: &str) -> anyhow::Result<Vec<Branch>> {
        let ids = branch_o2o_values::Entity::find()
            .filter(branch_o2o_values::Column::Name.eq(name))
            .filter(branch_o2o_values::Column::Value.eq(value))
            .all(&self.db)
            .await?
            .into_iter()
            .map(|r| r.branch_id)
            .collect();
        self.load_branches_by_ids(ids).await
    }

    /// Attach a single reference to an existing branch.
    pub async fn attach_reference(
        &self,
        path: &str,
        reference: &Reference,
    ) -> anyhow::Result<()> {
        let model = self
            .model_by_path(path)
            .await?
            .ok_or_else(|| anyhow::anyhow!("branch not found: {path}"))?;
        let am = branch_references::ActiveModel {
            id: NotSet,
            branch_id: Set(model.id),
            r#type: Set(reference.kind.clone()),
            value: Set(reference.value.clone()),
            note: Set(reference.note.clone()),
        };
        branch_references::Entity::insert(am).exec(&self.db).await?;
        Ok(())
    }

    /// `(path, title)` for every branch that has a title.
    ///
    /// A projection — it loads only the two columns the matcher needs, avoiding
    /// the full o2o/o2m/reference hydration of `find`.
    pub async fn titles(&self) -> anyhow::Result<Vec<(String, String)>> {
        #[derive(FromQueryResult)]
        struct TitleRow {
            path: String,
            title: Option<String>,
        }
        let rows = branches::Entity::find()
            .select_only()
            .column(branches::Column::Path)
            .column(branches::Column::Title)
            .into_model::<TitleRow>()
            .all(&self.db)
            .await?;
        Ok(rows
            .into_iter()
            .filter_map(|r| r.title.map(|t| (r.path, t)))
            .collect())
    }

    /// Branches carrying a specific o2m value.
    pub async fn branches_with_o2m(&self, name: &str, value: &str) -> anyhow::Result<Vec<Branch>> {
        let ids = branch_o2m_values::Entity::find()
            .filter(branch_o2m_values::Column::Name.eq(name))
            .filter(branch_o2m_values::Column::Value.eq(value))
            .all(&self.db)
            .await?
            .into_iter()
            .map(|r| r.branch_id)
            .collect();
        self.load_branches_by_ids(ids).await
    }
}

#[async_trait]
impl BranchRepository for SeaOrmBranchRepository {
    async fn add(&self, branch: Branch) -> anyhow::Result<()> {
        let now = crate::history::now();
        let txn = self.db.begin().await?;

        let am = mapper::branch_active_model(&branch, &now, &now);
        let res = branches::Entity::insert(am).exec(&txn).await?;
        let branch_id = res.last_insert_id;

        Self::insert_children(&txn, branch_id, &branch).await?;
        txn.commit().await?;
        Ok(())
    }

    async fn get(&self, path: &str) -> anyhow::Result<Option<Branch>> {
        match self.model_by_path(path).await? {
            Some(model) => Ok(Some(self.hydrate(model).await?)),
            None => Ok(None),
        }
    }

    async fn update(&self, branch: Branch) -> anyhow::Result<()> {
        let existing = self
            .model_by_path(&branch.path)
            .await?
            .ok_or_else(|| anyhow::anyhow!("branch not found: {}", branch.path))?;
        let branch_id = existing.id;
        let created_at = existing.created_at.clone();
        let now = crate::history::now();

        let txn = self.db.begin().await?;

        // Replace the row, preserving created_at and the existing id.
        let mut branch = branch;
        branch.id = Some(branch_id as i64);
        let am = mapper::branch_active_model(&branch, &created_at, &now);
        branches::Entity::update(am).exec(&txn).await?;

        Self::replace_children(&txn, branch_id, &branch).await?;

        txn.commit().await?;
        Ok(())
    }

    async fn delete(&self, path: &str) -> anyhow::Result<()> {
        let Some(model) = self.model_by_path(path).await? else {
            return Ok(());
        };
        let branch_id = model.id;
        let txn = self.db.begin().await?;

        // Explicit child deletes so behavior does not depend on the
        // foreign_keys pragma being set on every pooled connection.
        branch_o2o_values::Entity::delete_many()
            .filter(branch_o2o_values::Column::BranchId.eq(branch_id))
            .exec(&txn)
            .await?;
        branch_o2m_values::Entity::delete_many()
            .filter(branch_o2m_values::Column::BranchId.eq(branch_id))
            .exec(&txn)
            .await?;
        branch_references::Entity::delete_many()
            .filter(branch_references::Column::BranchId.eq(branch_id))
            .exec(&txn)
            .await?;
        branches::Entity::delete_by_id(branch_id).exec(&txn).await?;

        txn.commit().await?;
        Ok(())
    }

    async fn children(&self, path: &str) -> anyhow::Result<Vec<Branch>> {
        let models = branches::Entity::find()
            .filter(branches::Column::ParentPath.eq(path))
            .all(&self.db)
            .await?;
        self.hydrate_all(models).await
    }

    async fn descendants(&self, path: &str) -> anyhow::Result<Vec<Branch>> {
        let models = branches::Entity::find()
            .filter(branches::Column::Path.like(descendant_like(path)))
            .all(&self.db)
            .await?;
        self.hydrate_all(models).await
    }

    async fn ancestors(&self, path: &str) -> anyhow::Result<Vec<Branch>> {
        let mut paths = Vec::new();
        let mut current = path.to_string();
        while let Some(parent) = parent_of(&current) {
            paths.push(parent.clone());
            current = parent;
        }
        if paths.is_empty() {
            return Ok(Vec::new());
        }
        let models = branches::Entity::find()
            .filter(branches::Column::Path.is_in(paths))
            .all(&self.db)
            .await?;
        self.hydrate_all(models).await
    }

    async fn find(&self, query: SampleQuery) -> anyhow::Result<Vec<Branch>> {
        // All hard filters (scope + require/avoid o2o/o2m) run in SQL against the
        // indexed child tables, so only matching rows are hydrated. Soft scoring
        // is the sampler's job (Phase 7).
        let mut select = branches::Entity::find();
        if let Some(under) = &query.under {
            let under = under.trim_end_matches('/').to_string();
            select = select.filter(
                branches::Column::Path
                    .eq(under.clone())
                    .or(branches::Column::Path.like(descendant_like(&under))),
            );
        }
        for (name, value) in &query.require_o2o {
            select = select.filter(branches::Column::Id.in_subquery(o2o_subquery(name, value)));
        }
        for (name, value) in &query.avoid_o2o {
            select = select.filter(branches::Column::Id.not_in_subquery(o2o_subquery(name, value)));
        }
        for (name, values) in &query.avoid_o2m {
            select = select.filter(branches::Column::Id.not_in_subquery(o2m_subquery(name, values)));
        }
        let models = select.all(&self.db).await?;
        self.hydrate_all(models).await
    }

    /// Atomic batch upsert: the whole set is applied in one transaction, so a
    /// failure on any branch rolls the entire import back.
    async fn upsert_many(&self, branches: Vec<Branch>) -> anyhow::Result<(usize, usize)> {
        let now = crate::history::now();
        let txn = self.db.begin().await?;
        let (mut added, mut updated) = (0usize, 0usize);

        for mut branch in branches {
            let existing = branches::Entity::find()
                .filter(branches::Column::Path.eq(&branch.path))
                .one(&txn)
                .await?;
            match existing {
                Some(model) => {
                    let branch_id = model.id;
                    branch.id = Some(branch_id as i64);
                    let am = mapper::branch_active_model(&branch, &model.created_at, &now);
                    branches::Entity::update(am).exec(&txn).await?;
                    Self::replace_children(&txn, branch_id, &branch).await?;
                    updated += 1;
                }
                None => {
                    branch.id = None;
                    let am = mapper::branch_active_model(&branch, &now, &now);
                    let branch_id = branches::Entity::insert(am).exec(&txn).await?.last_insert_id;
                    Self::insert_children(&txn, branch_id, &branch).await?;
                    added += 1;
                }
            }
        }

        txn.commit().await?;
        Ok((added, updated))
    }
}

/// Build a `LIKE` pattern matching the strict descendants of `scope`.
///
/// Paths may legitimately contain `_` and `%`, which are SQL `LIKE`
/// metacharacters; without escaping, a scope like `/a_b` would also match
/// `/axb/...`. We escape `%`, `_`, and the `\` escape character itself and
/// emit an explicit `ESCAPE '\'` clause so only real descendants match.
fn descendant_like(scope: &str) -> LikeExpr {
    let scope = scope.trim_end_matches('/');
    let mut pattern = String::with_capacity(scope.len() + 2);
    for ch in scope.chars() {
        if matches!(ch, '\\' | '%' | '_') {
            pattern.push('\\');
        }
        pattern.push(ch);
    }
    pattern.push_str("/%");
    LikeExpr::new(pattern).escape('\\')
}

/// Subquery selecting branch ids that carry the o2o pair `name=value`.
fn o2o_subquery(name: &str, value: &str) -> SelectStatement {
    Query::select()
        .column(branch_o2o_values::Column::BranchId)
        .from(branch_o2o_values::Entity)
        .and_where(branch_o2o_values::Column::Name.eq(name))
        .and_where(branch_o2o_values::Column::Value.eq(value))
        .to_owned()
}

/// Subquery selecting branch ids that carry any of `values` under o2m `name`.
///
/// An empty `values` yields an empty result set, so `NOT IN (…)` keeps every
/// branch — matching "avoid nothing".
fn o2m_subquery(name: &str, values: &[String]) -> SelectStatement {
    Query::select()
        .column(branch_o2m_values::Column::BranchId)
        .from(branch_o2m_values::Entity)
        .and_where(branch_o2m_values::Column::Name.eq(name))
        .and_where(branch_o2m_values::Column::Value.is_in(values.to_vec()))
        .to_owned()
}
