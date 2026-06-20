//! Repository trait and SeaORM-backed implementation for branches.

use async_trait::async_trait;
use sea_orm::{
    ColumnTrait, DatabaseConnection, DatabaseTransaction, DbBackend, EntityTrait, FromQueryResult,
    QueryFilter, Statement, TransactionTrait,
};
use unclip_core::{parent_of, Branch, SampleQuery};
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

    /// Load a branch's child rows and assemble the full domain value.
    async fn hydrate(&self, model: branches::Model) -> anyhow::Result<Branch> {
        let id = model.id;
        let o2o = branch_o2o_values::Entity::find()
            .filter(branch_o2o_values::Column::BranchId.eq(id))
            .all(&self.db)
            .await?;
        let o2m = branch_o2m_values::Entity::find()
            .filter(branch_o2m_values::Column::BranchId.eq(id))
            .all(&self.db)
            .await?;
        let refs = branch_references::Entity::find()
            .filter(branch_references::Column::BranchId.eq(id))
            .all(&self.db)
            .await?;
        mapper::assemble_branch(model, o2o, o2m, refs)
    }

    async fn hydrate_all(&self, models: Vec<branches::Model>) -> anyhow::Result<Vec<Branch>> {
        let mut out = Vec::with_capacity(models.len());
        for model in models {
            out.push(self.hydrate(model).await?);
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
        let now = chrono::Utc::now().to_rfc3339();
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
        let now = chrono::Utc::now().to_rfc3339();

        let txn = self.db.begin().await?;

        // Replace the row, preserving created_at and the existing id.
        let mut branch = branch;
        branch.id = Some(branch_id as i64);
        let am = mapper::branch_active_model(&branch, &created_at, &now);
        branches::Entity::update(am).exec(&txn).await?;

        // Replace child rows wholesale.
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
        Self::insert_children(&txn, branch_id, &branch).await?;

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
        let prefix = format!("{}/%", path.trim_end_matches('/'));
        let models = branches::Entity::find()
            .filter(branches::Column::Path.like(prefix))
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
        // Scope filter in SQL; hard o2o/o2m filters applied in Rust over the
        // hydrated candidates. Soft scoring is the sampler's job (Phase 7).
        let mut select = branches::Entity::find();
        if let Some(under) = &query.under {
            let under = under.trim_end_matches('/').to_string();
            let prefix = format!("{under}/%");
            select = select.filter(
                branches::Column::Path
                    .eq(under.clone())
                    .or(branches::Column::Path.like(prefix)),
            );
        }
        let models = select.all(&self.db).await?;
        let candidates = self.hydrate_all(models).await?;

        let kept = candidates
            .into_iter()
            .filter(|b| matches_hard_filters(b, &query))
            .collect();
        Ok(kept)
    }
}

/// Apply the hard require/avoid filters (scope is already applied in SQL).
fn matches_hard_filters(branch: &Branch, query: &SampleQuery) -> bool {
    for (name, value) in &query.require_o2o {
        if branch.o2o.get(name) != Some(value) {
            return false;
        }
    }
    for (name, value) in &query.avoid_o2o {
        if branch.o2o.get(name) == Some(value) {
            return false;
        }
    }
    for (name, avoided) in &query.avoid_o2m {
        if let Some(values) = branch.o2m.get(name) {
            if avoided.iter().any(|v| values.contains(v)) {
                return false;
            }
        }
    }
    true
}
