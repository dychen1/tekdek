//! Syntax Fact Builder
//!
//! `SyntaxFactBuilder` creates entity, relation, and import-site facts.
//! It assigns stable entity keys and merges repeated fields.
//! Each fact list retains insertion order.

use std::collections::HashMap;

use crate::analysis::{
    fact::{
        AnalysisError, EntityEvidence, EntityFact, EntityKey, EntityKind, FieldData, ImportSite,
        RelationEvidence, RelationFact, RelationKind, SyntaxFacts,
    },
    source::{SourceRange, SourceSpan},
};

pub struct SyntaxFactBuilder {
    entities: Vec<EntityFact>,
    relations: Vec<RelationFact>,
    import_sites: Vec<ImportSite>,
    entity_positions: HashMap<EntityKey, usize>,
}

impl SyntaxFactBuilder {
    #[must_use]
    pub fn new() -> Self {
        Self {
            entities: Vec::new(),
            relations: Vec::new(),
            import_sites: Vec::new(),
            entity_positions: HashMap::new(),
        }
    }

    /// Adds one module under its relative source path.
    ///
    /// The module scope and body cover the whole source. Content hashes and
    /// position lookups rely on this rule.
    ///
    /// # Errors
    ///
    /// Returns an error if the path already names a module or the source is
    /// too large.
    pub fn add_module(&mut self, path: &str, source: &str) -> Result<EntityKey, AnalysisError> {
        let key = EntityKey::module(path);
        if self.entity_positions.contains_key(&key) {
            return Err(AnalysisError::new(format!(
                "duplicate source module: {key}"
            )));
        }
        let end = u32::try_from(source.len())
            .map_err(|_| AnalysisError::new(format!("the {path} source is too large")))?;
        let whole_source = Some(SourceRange { start: 0, end });
        self.push_entity(
            key.clone(),
            path.to_owned(),
            EntityKind::Module,
            EntityEvidence {
                path: path.to_owned(),
                declarations: Vec::new(),
                scope: whole_source,
                body: whole_source,
                type_inputs: Vec::new(),
                type_outputs: Vec::new(),
            },
        );
        Ok(key)
    }

    /// Adds one entity inside another entity.
    ///
    /// Repeated sibling names receive ordinals.
    pub fn add_entity(
        &mut self,
        owner: &EntityKey,
        name: String,
        kind: EntityKind,
        evidence: EntityEvidence,
    ) -> EntityKey {
        let key = self.next_entity_key(owner, &kind, &name);
        self.push_entity(key.clone(), name, kind, evidence);
        key
    }

    /// Adds or extends one field.
    ///
    /// The first declared type wins. Distinct declaration ranges accumulate.
    pub fn add_or_extend_field(
        &mut self,
        owner: &EntityKey,
        name: &str,
        path: &str,
        declared_type: Option<String>,
        declaration: SourceRange,
    ) -> EntityKey {
        let field = EntityKind::Field(FieldData { declared_type });
        let key = owner.child(&field, name, 0);

        if self.entity_positions.contains_key(&key) {
            let EntityKind::Field(data) = field else {
                unreachable!("the field kind was built one line above");
            };
            let (kind, evidence) = self.entity_kind_and_evidence_mut(&key);
            if let EntityKind::Field(existing) = kind
                && existing.declared_type.is_none()
            {
                existing.declared_type = data.declared_type;
            }
            if !evidence.declarations.contains(&declaration) {
                evidence.declarations.push(declaration);
            }
            return key;
        }

        self.push_entity(
            key.clone(),
            name.to_owned(),
            field,
            EntityEvidence {
                path: path.to_owned(),
                declarations: vec![declaration],
                scope: None,
                body: None,
                type_inputs: Vec::new(),
                type_outputs: Vec::new(),
            },
        );
        self.add_relation(owner, &key, RelationKind::Contains);
        key
    }

    /// Adds one relation without an observation.
    pub fn add_relation(&mut self, source: &EntityKey, target: &EntityKey, kind: RelationKind) {
        self.relations.push(RelationFact {
            source: source.clone(),
            target: target.clone(),
            kind,
            evidence: RelationEvidence {
                observations: Vec::new(),
            },
        });
    }

    /// Adds one import statement with its owner and imported names.
    pub fn add_import_site(
        &mut self,
        owner: &EntityKey,
        statement: SourceSpan,
        names: Vec<SourceRange>,
    ) {
        self.import_sites.push(ImportSite {
            owner: owner.clone(),
            statement,
            names,
        });
    }

    /// Returns all fact lists and consumes the builder.
    #[must_use]
    pub fn finish(self) -> SyntaxFacts {
        SyntaxFacts {
            entities: self.entities,
            relations: self.relations,
            import_sites: self.import_sites,
        }
    }

    /// Returns the first unused key for one same-named sibling group.
    fn next_entity_key(&self, owner: &EntityKey, kind: &EntityKind, name: &str) -> EntityKey {
        let mut ordinal = 0;
        loop {
            let key = owner.child(kind, name, ordinal);
            if !self.entity_positions.contains_key(&key) {
                return key;
            }
            ordinal += 1;
        }
    }

    /// Adds one entity fact and indexes its position.
    fn push_entity(
        &mut self,
        key: EntityKey,
        name: String,
        kind: EntityKind,
        evidence: EntityEvidence,
    ) {
        self.entity_positions
            .insert(key.clone(), self.entities.len());
        self.entities.push(EntityFact {
            key,
            name,
            kind,
            evidence,
        });
    }

    fn entity_kind_and_evidence_mut(
        &mut self,
        key: &EntityKey,
    ) -> (&mut EntityKind, &mut EntityEvidence) {
        let position = self.entity_positions[key];
        let entity = &mut self.entities[position];
        (&mut entity.kind, &mut entity.evidence)
    }
}

impl Default for SyntaxFactBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use crate::analysis::fact::{EntityEvidence, EntityKind, TypeData, TypeKind};

    use super::SyntaxFactBuilder;

    #[test]
    fn assigns_ordinals_after_entity_key_collisions() {
        let mut facts = SyntaxFactBuilder::new();
        let module = facts
            .add_module("app.py", "")
            .expect("the module key must be unique");

        let first = facts.add_entity(&module, "Router".to_owned(), class(), evidence());
        let second = facts.add_entity(&module, "Router".to_owned(), class(), evidence());

        assert_eq!(first.as_str(), "app.py::type:Router");
        assert_eq!(second.as_str(), "app.py::type:Router[1]");
    }

    fn class() -> EntityKind {
        EntityKind::Type(TypeData {
            kind: TypeKind::Class,
            documentation: None,
        })
    }

    fn evidence() -> EntityEvidence {
        EntityEvidence {
            path: "app.py".to_owned(),
            declarations: Vec::new(),
            scope: None,
            body: None,
            type_inputs: Vec::new(),
            type_outputs: Vec::new(),
        }
    }
}
