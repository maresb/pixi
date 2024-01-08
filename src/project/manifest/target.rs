use crate::project::manifest::activation::Activation;
use crate::utils::spanned::PixiSpanned;
use crate::{
    project::{manifest::error::SpecIsMissing, manifest::PyPiRequirement, SpecType},
    task::Task,
};
use indexmap::map::Entry;
use indexmap::IndexMap;
use itertools::Either;
use rattler_conda_types::{NamelessMatchSpec, PackageName, Platform};
use serde::{Deserialize, Deserializer};
use serde_with::{serde_as, DisplayFromStr, PickFirst};
use std::collections::HashMap;
use std::str::FromStr;

/// A target describes the dependencies, activations and task available to a specific feature, in
/// a specific environment, and optionally for a specific platform.
#[derive(Default, Debug, Clone)]
pub struct Target {
    /// Dependencies for this target.
    pub dependencies: HashMap<SpecType, IndexMap<PackageName, NamelessMatchSpec>>,

    /// Specific python dependencies
    pub pypi_dependencies: Option<IndexMap<rip::types::PackageName, PyPiRequirement>>,

    /// Additional information to activate an environment.
    pub activation: Option<Activation>,

    /// Target specific tasks to run in the environment
    pub tasks: HashMap<String, Task>,
}

impl Target {
    /// Returns the run dependencies of the target
    pub fn run_dependencies(&self) -> Option<&IndexMap<PackageName, NamelessMatchSpec>> {
        self.dependencies.get(&SpecType::Run)
    }

    /// Returns the host dependencies of the target
    pub fn host_dependencies(&self) -> Option<&IndexMap<PackageName, NamelessMatchSpec>> {
        self.dependencies.get(&SpecType::Host)
    }

    /// Returns the build dependencies of the target
    pub fn build_dependencies(&self) -> Option<&IndexMap<PackageName, NamelessMatchSpec>> {
        self.dependencies.get(&SpecType::Build)
    }

    /// Removes a dependency from this target.
    pub fn remove_dependency(
        &mut self,
        dep_str: &str,
        spec_type: SpecType,
    ) -> Result<(PackageName, NamelessMatchSpec), SpecIsMissing> {
        let Some(dependencies) = self.dependencies.get_mut(&spec_type) else {
            return Err(SpecIsMissing::spec_type_is_missing(dep_str, spec_type));
        };

        PackageName::from_str(dep_str)
            .map_err(|_| SpecIsMissing::dep_is_missing(dep_str, spec_type))
            .and_then(|dep| {
                dependencies
                    .shift_remove_entry(&dep)
                    .ok_or_else(|| SpecIsMissing::dep_is_missing(dep_str, spec_type))
            })
    }
}

/// Represents a target selector. Currently we only support explicit platform selection.
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub enum TargetSelector {
    // Platform specific configuration
    Platform(Platform),
    // TODO: Add minijinja coolness here.
}

impl TargetSelector {
    /// Returns true if this selector matches the given platform.
    pub fn matches(&self, platform: Platform) -> bool {
        match self {
            TargetSelector::Platform(p) => p == &platform,
        }
    }
}

impl ToString for TargetSelector {
    fn to_string(&self) -> String {
        match self {
            TargetSelector::Platform(p) => p.to_string(),
        }
    }
}

impl From<Platform> for TargetSelector {
    fn from(value: Platform) -> Self {
        TargetSelector::Platform(value)
    }
}

impl<'de> Deserialize<'de> for TargetSelector {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Ok(TargetSelector::Platform(Platform::deserialize(
            deserializer,
        )?))
    }
}

impl<'de> Deserialize<'de> for Target {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[serde_as]
        #[derive(Debug, Clone, Default, Deserialize)]
        #[serde(rename_all = "kebab-case")]
        #[serde(deny_unknown_fields)]
        pub struct TomlTarget {
            #[serde(default)]
            #[serde_as(as = "IndexMap<_, PickFirst<(DisplayFromStr, _)>>")]
            dependencies: IndexMap<PackageName, NamelessMatchSpec>,

            #[serde(default)]
            #[serde_as(as = "Option<IndexMap<_, PickFirst<(DisplayFromStr, _)>>>")]
            host_dependencies: Option<IndexMap<PackageName, NamelessMatchSpec>>,

            #[serde(default)]
            #[serde_as(as = "Option<IndexMap<_, PickFirst<(DisplayFromStr, _)>>>")]
            build_dependencies: Option<IndexMap<PackageName, NamelessMatchSpec>>,

            #[serde(default)]
            pypi_dependencies: Option<IndexMap<rip::types::PackageName, PyPiRequirement>>,

            /// Additional information to activate an environment.
            #[serde(default)]
            activation: Option<Activation>,

            /// Target specific tasks to run in the environment
            #[serde(default)]
            tasks: HashMap<String, Task>,
        }

        let target = TomlTarget::deserialize(deserializer)?;

        let mut dependencies = HashMap::from_iter([(SpecType::Run, target.dependencies)]);
        if let Some(host_deps) = target.host_dependencies {
            dependencies.insert(SpecType::Host, host_deps);
        }
        if let Some(build_deps) = target.build_dependencies {
            dependencies.insert(SpecType::Build, build_deps);
        }

        Ok(Self {
            dependencies,
            pypi_dependencies: target.pypi_dependencies,
            activation: target.activation,
            tasks: target.tasks,
        })
    }
}

/// A collect of targets including a default target.
#[derive(Debug, Clone, Default)]
pub struct Targets {
    default_target: Target,

    /// We use an [`IndexMap`] to preserve the order in which the items where defined in the
    /// manifest.
    targets: IndexMap<TargetSelector, Target>,

    /// The source location of the target selector in the manifest.
    source_locs: HashMap<TargetSelector, std::ops::Range<usize>>,
}

impl Targets {
    /// Constructs a new [`Targets`] from a default target and additional user defined targets.
    pub fn from_default_and_user_defined(
        default_target: Target,
        user_defined_targets: IndexMap<PixiSpanned<TargetSelector>, Target>,
    ) -> Self {
        let mut targets = IndexMap::with_capacity(user_defined_targets.len());
        let mut source_locs = HashMap::with_capacity(user_defined_targets.len());
        for (selector, target) in user_defined_targets {
            targets.insert(selector.value.clone(), target);
            if let Some(span) = selector.span {
                source_locs.insert(selector.value, span);
            }
        }

        Self {
            default_target,
            targets,
            source_locs,
        }
    }

    /// Returns the default target.
    pub fn default(&self) -> &Target {
        &self.default_target
    }

    /// Returns the default target
    pub fn default_mut(&mut self) -> &mut Target {
        &mut self.default_target
    }

    /// Returns all the targets that apply for the given platform. If no platform is specified, only
    /// the default target is returned.
    ///
    /// Multiple selectors might match for a given platform. This function returns all of them in
    /// order, with the most specific selector first and the default target last.
    ///
    /// This also always includes the default target.
    pub fn resolve(&self, platform: Option<Platform>) -> impl Iterator<Item = &'_ Target> + '_ {
        if let Some(platform) = platform {
            Either::Left(self.resolve_for_platform(platform))
        } else {
            Either::Right(std::iter::once(&self.default_target))
        }
    }

    /// Returns all the targets that apply for the given platform.
    ///
    /// Multiple selectors might match for a given platform. This function returns all of them in
    /// order, with the most specific selector first and the default target last.
    ///
    /// This also always includes the default target.
    ///
    /// You should use the [`Self::resolve`] function.
    fn resolve_for_platform(&self, platform: Platform) -> impl Iterator<Item = &'_ Target> + '_ {
        std::iter::once(&self.default_target)
            .chain(self.targets.iter().filter_map(move |(selector, target)| {
                if selector.matches(platform) {
                    Some(target)
                } else {
                    None
                }
            }))
            // We reverse this to get the most specific selector first.
            .rev()
    }

    /// Returns the target for the given target selector.
    pub fn for_target(&self, target: &TargetSelector) -> Option<&Target> {
        self.targets.get(target)
    }

    /// Returns the target for the given target selector or the default target if the selector is
    /// `None`.
    pub fn for_opt_target(&self, target: Option<&TargetSelector>) -> Option<&Target> {
        if let Some(sel) = target {
            self.targets.get(sel)
        } else {
            Some(&self.default_target)
        }
    }

    /// Returns the target for the given target selector or the default target if no target is
    /// specified.
    pub fn for_opt_target_mut(&mut self, target: Option<&TargetSelector>) -> Option<&mut Target> {
        if let Some(sel) = target {
            self.targets.get_mut(sel)
        } else {
            Some(&mut self.default_target)
        }
    }

    /// Returns the target for the given target selector or the default target if no target is
    /// specified.
    ///
    /// If a target is specified and it does not exist the default target is returned instead.
    pub fn for_opt_target_or_default(&self, target: Option<&TargetSelector>) -> &Target {
        if let Some(sel) = target {
            self.targets.get(sel).unwrap_or(&self.default_target)
        } else {
            &self.default_target
        }
    }

    /// Returns a mutable reference to the target for the given target selector or the default
    /// target if no target is specified.
    ///
    /// If a target is specified and it does not exist, it will be created.
    pub fn for_opt_target_or_default_mut(
        &mut self,
        target: Option<&TargetSelector>,
    ) -> &mut Target {
        if let Some(sel) = target {
            self.targets.entry(sel.clone()).or_default()
        } else {
            &mut self.default_target
        }
    }

    /// Returns the target for the given target selector.
    pub fn target_entry(&mut self, selector: TargetSelector) -> Entry<'_, TargetSelector, Target> {
        self.targets.entry(selector)
    }

    /// Returns an iterator over all targets and selectors.
    pub fn iter(&self) -> impl Iterator<Item = (&'_ Target, Option<&'_ TargetSelector>)> + '_ {
        std::iter::once((&self.default_target, None))
            .chain(self.targets.iter().map(|(sel, target)| (target, Some(sel))))
    }

    /// Returns an iterator over all targets.
    pub fn targets(&self) -> impl Iterator<Item = &'_ Target> + '_ {
        std::iter::once(&self.default_target).chain(self.targets.iter().map(|(_, target)| target))
    }

    /// Returns user defined target selectors
    pub fn user_defined_selectors(&self) -> impl Iterator<Item = &TargetSelector> + '_ {
        self.targets.keys()
    }

    /// Returns the source location of the target selector in the manifest.
    pub fn source_loc(&self, selector: &TargetSelector) -> Option<std::ops::Range<usize>> {
        self.source_locs.get(selector).cloned()
    }
}