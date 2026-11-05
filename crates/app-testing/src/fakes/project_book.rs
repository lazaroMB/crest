//! The projects of the in-memory session store: the same rules as the SQLite
//! adapter, from `domain::project`.

use domain::conversation::ProjectId;
use domain::project::Project;

use crate::ports::store::StoreError;

/// Projects in creation order.
#[derive(Debug, Default)]
pub struct ProjectBook {
    projects: Vec<Project>,
}

impl ProjectBook {
    /// Every project, oldest first.
    pub fn all(&self) -> Vec<Project> {
        self.projects.clone()
    }

    /// Whether a project has this id.
    pub fn contains(&self, id: ProjectId) -> bool {
        self.projects.iter().any(|p| p.id == id)
    }

    /// Creates a project.
    pub fn create(&mut self, name: &str, roots: &[String]) -> Result<Project, StoreError> {
        let project = Project::new(name, roots)?;
        self.projects.push(project.clone());
        Ok(project)
    }

    /// Applies `change` to project `id`; nothing is kept when it fails.
    pub fn edit(
        &mut self,
        id: ProjectId,
        change: impl FnOnce(&mut Project) -> Result<(), domain::project::ProjectError>,
    ) -> Result<Project, StoreError> {
        let project = self
            .projects
            .iter_mut()
            .find(|p| p.id == id)
            .ok_or(StoreError::ProjectNotFound(id))?;
        let mut edited = project.clone();
        change(&mut edited)?;
        *project = edited.clone();
        Ok(edited)
    }
}
