use crate::{Children, HierarchyEvent, Parent};
use bevy_ecs::{
    bundle::Bundle,
    entity::Entity,
    event::Events,
    system::{Command, Commands, EntityCommands},
    world::{EntityWorldMut, World},
};
use smallvec::{smallvec, SmallVec};

// Do not use `world.send_event_batch` as it prints error message when the Events are not available in the world,
// even though it's a valid use case to execute commands on a world without events. Loading a GLTF file for example
fn push_events(world: &mut World, events: impl IntoIterator<Item = HierarchyEvent>) {
    if let Some(mut moved) = world.get_resource_mut::<Events<HierarchyEvent>>() {
        moved.extend(events);
    }
}

/// Adds `child` to `parent`'s [`Children`], without checking if it is already present there.
///
/// This might cause unexpected results when removing duplicate children.
fn add_child_unchecked(world: &mut World, parent: Entity, child: Entity) {
    let mut parent = world.entity_mut(parent);
    if let Some(mut children) = parent.get_mut::<Children>() {
        children.0.push(child);
    } else {
        parent.insert(Children(smallvec![child]));
    }
}

/// Sets [`Parent`] of the `child` to `new_parent`. Inserts [`Parent`] if `child` doesn't have one.
fn update_parent(world: &mut World, child: Entity, new_parent: Entity) -> Option<Entity> {
    let mut child = world.entity_mut(child);
    if let Some(mut parent) = child.get_mut::<Parent>() {
        let previous = parent.0;
        *parent = Parent(new_parent);
        Some(previous)
    } else {
        child.insert(Parent(new_parent));
        None
    }
}

/// Remove child from the parent's [`Children`] component.
///
/// Removes the [`Children`] component from the parent if it's empty.
fn remove_from_children(world: &mut World, parent: Entity, child: Entity) {
    let Ok(mut parent) = world.get_entity_mut(parent) else {
        return;
    };
    let Some(mut children) = parent.get_mut::<Children>() else {
        return;
    };
    children.0.retain(|x| *x != child);
    if children.is_empty() {
        parent.remove::<Children>();
    }
}

/// Update the [`Parent`] component of the `child`.
/// Removes the `child` from the previous parent's [`Children`].
///
/// Does not update the new parents [`Children`] component.
///
/// Does nothing if `child` was already a child of `parent`.
///
/// Sends [`HierarchyEvent`]'s.
fn update_old_parent(world: &mut World, child: Entity, parent: Entity) {
    let previous = update_parent(world, child, parent);
    if let Some(previous_parent) = previous {
        // Do nothing if the child was already parented to this entity.
        if previous_parent == parent {
            return;
        }
        remove_from_children(world, previous_parent, child);

        push_events(
            world,
            [HierarchyEvent::ChildMoved {
                child,
                previous_parent,
                new_parent: parent,
            }],
        );
    } else {
        push_events(world, [HierarchyEvent::ChildAdded { child, parent }]);
    }
}

/// Update the [`Parent`] components of the `children`.
/// Removes the `children` from their previous parent's [`Children`].
///
/// Does not update the new parents [`Children`] component.
///
/// Does nothing for a child if it was already a child of `parent`.
///
/// Sends [`HierarchyEvent`]'s.
fn update_old_parents(world: &mut World, parent: Entity, children: &[Entity]) {
    let mut events: SmallVec<[HierarchyEvent; 8]> = SmallVec::with_capacity(children.len());
    for &child in children {
        if let Some(previous) = update_parent(world, child, parent) {
            // Do nothing if the entity already has the correct parent.
            if parent == previous {
                continue;
            }

            remove_from_children(world, previous, child);
            events.push(HierarchyEvent::ChildMoved {
                child,
                previous_parent: previous,
                new_parent: parent,
            });
        } else {
            events.push(HierarchyEvent::ChildAdded { child, parent });
        }
    }
    push_events(world, events);
}

/// Removes entities in `children` from `parent`'s [`Children`], removing the component if it ends up empty.
/// Also removes [`Parent`] component from `children`.
fn remove_children(parent: Entity, children: &[Entity], world: &mut World) {
    let mut events: SmallVec<[HierarchyEvent; 8]> = SmallVec::new();
    if let Some(parent_children) = world.get::<Children>(parent) {
        for &child in children {
            if parent_children.contains(&child) {
                events.push(HierarchyEvent::ChildRemoved { child, parent });
            }
        }
    } else {
        return;
    }
    for event in &events {
        if let &HierarchyEvent::ChildRemoved { child, .. } = event {
            world.entity_mut(child).remove::<Parent>();
        }
    }
    push_events(world, events);

    let mut parent = world.entity_mut(parent);
    if let Some(mut parent_children) = parent.get_mut::<Children>() {
        parent_children
            .0
            .retain(|parent_child| !children.contains(parent_child));

        if parent_children.is_empty() {
            parent.remove::<Children>();
        }
    }
}

/// Struct for building children entities and adding them to a parent entity.
///
/// # Example
///
/// This example creates three entities, a parent and two children.
///
/// ```
/// # use bevy_ecs::bundle::Bundle;
/// # use bevy_ecs::system::Commands;
/// # use bevy_hierarchy::{ChildBuild, BuildChildren};
/// # #[derive(Bundle)]
/// # struct MyBundle {}
/// # #[derive(Bundle)]
/// # struct MyChildBundle {}
/// #
/// # fn test(mut commands: Commands) {
/// commands.spawn(MyBundle {}).with_children(|child_builder| {
///     child_builder.spawn(MyChildBundle {});
///     child_builder.spawn(MyChildBundle {});
/// });
/// # }
/// ```
pub struct ChildBuilder<'a> {
    commands: Commands<'a, 'a>,
    children: SmallVec<[Entity; 8]>,
    parent: Entity,
}

/// Trait for building children entities and adding them to a parent entity. This is used in
/// implementations of [`BuildChildren`] as a bound on the [`Builder`](BuildChildren::Builder)
/// associated type. The closure passed to [`BuildChildren::with_children`] accepts an
/// implementation of `ChildBuild` so that children can be spawned via [`ChildBuild::spawn`].
pub trait ChildBuild {
    /// Spawn output type. Both [`spawn`](Self::spawn) and [`spawn_empty`](Self::spawn_empty) return
    /// an implementation of this type so that children can be operated on via method-chaining.
    /// Implementations of `ChildBuild` reborrow `self` when spawning entities (see
    /// [`Commands::spawn_empty`] and [`World::get_entity_mut`]). Lifetime `'a` corresponds to this
    /// reborrowed self, and `Self` outlives it.
    type SpawnOutput<'a>: BuildChildren
    where
        Self: 'a;

    /// Spawns an entity with the given bundle and inserts it into the parent entity's [`Children`].
    /// Also adds [`Parent`] component to the created entity.
    fn spawn(&mut self, bundle: impl Bundle) -> Self::SpawnOutput<'_>;

    /// Spawns an [`Entity`] with no components and inserts it into the parent entity's [`Children`].
    /// Also adds [`Parent`] component to the created entity.
    fn spawn_empty(&mut self) -> Self::SpawnOutput<'_>;

    /// Returns the parent entity.
    fn parent_entity(&self) -> Entity;

    /// Adds a command to be executed, like [`Commands::queue`].
    fn queue_command<C: Command>(&mut self, command: C) -> &mut Self;
}

impl ChildBuild for ChildBuilder<'_> {
    type SpawnOutput<'a>
        = EntityCommands<'a>
    where
        Self: 'a;

    fn spawn(&mut self, bundle: impl Bundle) -> EntityCommands {
        let e = self.commands.spawn(bundle);
        self.children.push(e.id());
        e
    }

    fn spawn_empty(&mut self) -> EntityCommands {
        let e = self.commands.spawn_empty();
        self.children.push(e.id());
        e
    }

    fn parent_entity(&self) -> Entity {
        self.parent
    }

    fn queue_command<C: Command>(&mut self, command: C) -> &mut Self {
        self.commands.queue(command);
        self
    }
}

/// Trait for removing, adding and replacing children and parents of an entity.
pub trait BuildChildren {
    /// Child builder type.
    type Builder<'a>: ChildBuild;

    /// Takes a closure which builds children for this entity using [`ChildBuild`].
    ///
    /// For convenient spawning of a single child, you can use [`with_child`].
    ///
    /// [`with_child`]: BuildChildren::with_child
    fn with_children(&mut self, f: impl FnOnce(&mut Self::Builder<'_>)) -> &mut Self;

    /// Spawns the passed bundle and adds it to this entity as a child.
    ///
    /// The bundle's [`Parent`] component will be updated to the new parent.
    ///
    /// For efficient spawning of multiple children, use [`with_children`].
    ///
    /// [`with_children`]: BuildChildren::with_children
    fn with_child<B: Bundle>(&mut self, bundle: B) -> &mut Self;

    /// Pushes children to the back of the builder's children. For any entities that are
    /// already a child of this one, this method does nothing.
    ///
    /// The children's [`Parent`] component will be updated to the new parent.
    ///
    /// If the children were previously children of another parent, that parent's [`Children`] component
    /// will have those children removed from its list. Removing all children from a parent causes its
    /// [`Children`] component to be removed from the entity.
    ///
    /// # Panics
    ///
    /// Panics if any of the children are the same as the parent.
    fn add_children(&mut self, children: &[Entity]) -> &mut Self;

    /// Inserts children at the given index.
    ///
    /// The children's [`Parent`] component will be updated to the new parent.
    ///
    /// If the children were previously children of another parent, that parent's [`Children`] component
    /// will have those children removed from its list. Removing all children from a parent causes its
    /// [`Children`] component to be removed from the entity.
    ///
    /// # Panics
    ///
    /// Panics if any of the children are the same as the parent.
    fn insert_children(&mut self, index: usize, children: &[Entity]) -> &mut Self;

    /// Removes the given children.
    ///
    /// The removed children will have their [`Parent`] component removed.
    ///
    /// Removing all children from a parent causes its [`Children`] component to be removed from the entity.
    fn remove_children(&mut self, children: &[Entity]) -> &mut Self;

    /// Adds a single child.
    ///
    /// The child's [`Parent`] component will be updated to the new parent.
    ///
    /// If the child was previously the child of another parent, that parent's [`Children`] component
    /// will have the child removed from its list. Removing all children from a parent causes its
    /// [`Children`] component to be removed from the entity.
    ///
    /// # Panics
    ///
    /// Panics if the child is the same as the parent.
    fn add_child(&mut self, child: Entity) -> &mut Self;

    /// Removes all children from this entity. The [`Children`] component and the children's [`Parent`] component will be removed.
    /// If the [`Children`] component is not present, this has no effect.
    fn clear_children(&mut self) -> &mut Self;

    /// Removes all current children from this entity, replacing them with the specified list of entities.
    ///
    /// The added children's [`Parent`] component will be updated to the new parent.
    /// The removed children will have their [`Parent`] component removed.
    ///
    /// # Panics
    ///
    /// Panics if any of the children are the same as the parent.
    fn replace_children(&mut self, children: &[Entity]) -> &mut Self;

    /// Sets the parent of this entity.
    ///
    /// If this entity already had a parent, the parent's [`Children`] component will have this
    /// child removed from its list. Removing all children from a parent causes its [`Children`]
    /// component to be removed from the entity.
    ///
    /// # Panics
    ///
    /// Panics if the parent is the same as the child.
    fn set_parent(&mut self, parent: Entity) -> &mut Self;

    /// Removes the [`Parent`] of this entity.
    ///
    /// Also removes this entity from its parent's [`Children`] component. Removing all children from a parent causes
    /// its [`Children`] component to be removed from the entity.
    fn remove_parent(&mut self) -> &mut Self;
}

impl BuildChildren for EntityCommands<'_> {
    type Builder<'a> = ChildBuilder<'a>;

    fn with_children(&mut self, spawn_children: impl FnOnce(&mut Self::Builder<'_>)) -> &mut Self {
        let parent = self.id();
        let mut builder = ChildBuilder {
            commands: self.commands(),
            children: SmallVec::default(),
            parent,
        };

        spawn_children(&mut builder);

        let children = builder.children;
        if children.contains(&parent) {
            panic!("Entity cannot be a child of itself.");
        }
        self.queue(move |entity: Entity, world: &mut World| {
            world.entity_mut(entity).add_children(&children);
        })
    }

    fn with_child<B: Bundle>(&mut self, bundle: B) -> &mut Self {
        let child = self.commands().spawn(bundle).id();
        self.queue(move |entity: Entity, world: &mut World| {
            world.entity_mut(entity).add_child(child);
        })
    }

    fn add_children(&mut self, children: &[Entity]) -> &mut Self {
        let parent = self.id();
        if children.contains(&parent) {
            panic!("Cannot add entity as a child of itself.");
        }
        let children = SmallVec::<[Entity; 8]>::from_slice(children);
        self.queue(move |entity: Entity, world: &mut World| {
            world.entity_mut(entity).add_children(&children);
        })
    }

    fn insert_children(&mut self, index: usize, children: &[Entity]) -> &mut Self {
        let parent = self.id();
        if children.contains(&parent) {
            panic!("Cannot insert entity as a child of itself.");
        }
        let children = SmallVec::<[Entity; 8]>::from_slice(children);
        self.queue(move |entity: Entity, world: &mut World| {
            world.entity_mut(entity).insert_children(index, &children);
        })
    }

    fn remove_children(&mut self, children: &[Entity]) -> &mut Self {
        let children = SmallVec::<[Entity; 8]>::from_slice(children);
        self.queue(move |entity: Entity, world: &mut World| {
            world.entity_mut(entity).remove_children(&children);
        })
    }

    fn add_child(&mut self, child: Entity) -> &mut Self {
        let parent = self.id();
        if child == parent {
            panic!("Cannot add entity as a child of itself.");
        }
        self.queue(move |entity: Entity, world: &mut World| {
            world.entity_mut(entity).add_child(child);
        })
    }

    fn clear_children(&mut self) -> &mut Self {
        self.queue(move |entity: Entity, world: &mut World| {
            world.entity_mut(entity).clear_children();
        })
    }

    fn replace_children(&mut self, children: &[Entity]) -> &mut Self {
        let parent = self.id();
        if children.contains(&parent) {
            panic!("Cannot replace entity as a child of itself.");
        }
        let children = SmallVec::<[Entity; 8]>::from_slice(children);
        self.queue(move |entity: Entity, world: &mut World| {
            world.entity_mut(entity).replace_children(&children);
        })
    }

    fn set_parent(&mut self, parent: Entity) -> &mut Self {
        let child = self.id();
        if child == parent {
            panic!("Cannot set parent to itself");
        }
        self.queue(move |entity: Entity, world: &mut World| {
            world.entity_mut(parent).add_child(entity);
        })
    }

    fn remove_parent(&mut self) -> &mut Self {
        self.queue(move |entity: Entity, world: &mut World| {
            world.entity_mut(entity).remove_parent();
        })
    }
}

/// Struct for adding children to an entity directly through the [`World`] for use in exclusive systems.
#[derive(Debug)]
pub struct WorldChildBuilder<'w> {
    world: &'w mut World,
    parent: Entity,
}

impl ChildBuild for WorldChildBuilder<'_> {
    type SpawnOutput<'a>
        = EntityWorldMut<'a>
    where
        Self: 'a;

    fn spawn(&mut self, bundle: impl Bundle) -> EntityWorldMut {
        let entity = self.world.spawn((bundle, Parent(self.parent))).id();
        add_child_unchecked(self.world, self.parent, entity);
        push_events(
            self.world,
            [HierarchyEvent::ChildAdded {
                child: entity,
                parent: self.parent,
            }],
        );
        self.world.entity_mut(entity)
    }

    fn spawn_empty(&mut self) -> EntityWorldMut {
        self.spawn(())
    }

    fn parent_entity(&self) -> Entity {
        self.parent
    }

    fn queue_command<C: Command>(&mut self, command: C) -> &mut Self {
        self.world.commands().queue(command);
        self
    }
}

impl WorldChildBuilder<'_> {
    /// Calls the world's [`World::flush`] to apply any commands
    /// queued by [`Self::queue_command`].
    pub fn flush_world(&mut self) {
        self.world.flush();
    }
}

impl BuildChildren for EntityWorldMut<'_> {
    type Builder<'a> = WorldChildBuilder<'a>;

    fn with_children(&mut self, spawn_children: impl FnOnce(&mut WorldChildBuilder)) -> &mut Self {
        let parent = self.id();
        self.world_scope(|world| {
            spawn_children(&mut WorldChildBuilder { world, parent });
        });
        self
    }

    fn with_child<B: Bundle>(&mut self, bundle: B) -> &mut Self {
        let parent = self.id();
        let child = self.world_scope(|world| world.spawn((bundle, Parent(parent))).id());
        if let Some(mut children_component) = self.get_mut::<Children>() {
            children_component.0.retain(|value| child != *value);
            children_component.0.push(child);
        } else {
            self.insert(Children::from_entities(&[child]));
        }
        self
    }

    fn add_child(&mut self, child: Entity) -> &mut Self {
        let parent = self.id();
        if child == parent {
            panic!("Cannot add entity as a child of itself.");
        }
        self.world_scope(|world| {
            update_old_parent(world, child, parent);
        });
        if let Some(mut children_component) = self.get_mut::<Children>() {
            children_component.0.retain(|value| child != *value);
            children_component.0.push(child);
        } else {
            self.insert(Children::from_entities(&[child]));
        }
        self
    }

    fn add_children(&mut self, children: &[Entity]) -> &mut Self {
        if children.is_empty() {
            return self;
        }

        let parent = self.id();
        if children.contains(&parent) {
            panic!("Cannot push entity as a child of itself.");
        }
        self.world_scope(|world| {
            update_old_parents(world, parent, children);
        });
        if let Some(mut children_component) = self.get_mut::<Children>() {
            children_component
                .0
                .retain(|value| !children.contains(value));
            children_component.0.extend(children.iter().cloned());
        } else {
            self.insert(Children::from_entities(children));
        }
        self
    }

    fn insert_children(&mut self, index: usize, children: &[Entity]) -> &mut Self {
        let parent = self.id();
        if children.contains(&parent) {
            panic!("Cannot insert entity as a child of itself.");
        }
        self.world_scope(|world| {
            update_old_parents(world, parent, children);
        });
        if let Some(mut children_component) = self.get_mut::<Children>() {
            children_component
                .0
                .retain(|value| !children.contains(value));
            children_component.0.insert_from_slice(index, children);
        } else {
            self.insert(Children::from_entities(children));
        }
        self
    }

    fn remove_children(&mut self, children: &[Entity]) -> &mut Self {
        let parent = self.id();
        self.world_scope(|world| {
            remove_children(parent, children, world);
        });
        self
    }

    fn set_parent(&mut self, parent: Entity) -> &mut Self {
        let child = self.id();
        self.world_scope(|world| {
            world.entity_mut(parent).add_child(child);
        });
        self
    }

    fn remove_parent(&mut self) -> &mut Self {
        let child = self.id();
        if let Some(parent) = self.take::<Parent>().map(|p| p.get()) {
            self.world_scope(|world| {
                remove_from_children(world, parent, child);
                push_events(world, [HierarchyEvent::ChildRemoved { child, parent }]);
            });
        }
        self
    }

    fn clear_children(&mut self) -> &mut Self {
        let parent = self.id();
        self.world_scope(|world| {
            if let Some(children) = world.entity_mut(parent).take::<Children>() {
                for &child in &children.0 {
                    world.entity_mut(child).remove::<Parent>();
                }
            }
        });
        self
    }

    fn replace_children(&mut self, children: &[Entity]) -> &mut Self {
        self.clear_children().add_children(children)
    }
}

#[cfg(test)]
mod tests {
    use super::{BuildChildren, ChildBuild};
    use crate::{
        components::{Children, Parent},
        HierarchyEvent::{self, ChildAdded, ChildMoved, ChildRemoved},
    };
    use alloc::{vec, vec::Vec};
    use smallvec::{smallvec, SmallVec};

    use bevy_ecs::{
        component::Component,
        entity::Entity,
        event::Events,
        system::Commands,
        world::{CommandQueue, World},
    };

    /// Assert the (non)existence and state of the child's [`Parent`] component.
    fn assert_parent(world: &World, child: Entity, parent: Option<Entity>) {
        assert_eq!(world.get::<Parent>(child).map(Parent::get), parent);
    }

    /// Assert the (non)existence and state of the parent's [`Children`] component.
    fn assert_children(world: &World, parent: Entity, children: Option<&[Entity]>) {
        assert_eq!(world.get::<Children>(parent).map(|c| &**c), children);
    }

    /// Assert the number of children in the parent's [`Children`] component if it exists.
    fn assert_num_children(world: &World, parent: Entity, num_children: usize) {
        assert_eq!(
            world.get::<Children>(parent).map(|c| c.len()).unwrap_or(0),
            num_children
        );
    }

    /// Used to omit a number of events that are not relevant to a particular test.
    fn omit_events(world: &mut World, number: usize) {
        let mut events_resource = world.resource_mut::<Events<HierarchyEvent>>();
        let mut events: Vec<_> = events_resource.drain().collect();
        events_resource.extend(events.drain(number..));
    }

    fn assert_events(world: &mut World, expected_events: &[HierarchyEvent]) {
        let events: Vec<_> = world
            .resource_mut::<Events<HierarchyEvent>>()
            .drain()
            .collect();
        assert_eq!(events, expected_events);
    }

    #[test]
    fn add_child() {
        let world = &mut World::new();
        world.insert_resource(Events::<HierarchyEvent>::default());

        let [a, b, c, d] = core::array::from_fn(|_| world.spawn_empty().id());

        world.entity_mut(a).add_child(b);

        assert_parent(world, b, Some(a));
        assert_children(world, a, Some(&[b]));
        assert_events(
            world,
            &[ChildAdded {
                child: b,
                parent: a,
            }],
        );

        world.entity_mut(a).add_child(c);

        assert_children(world, a, Some(&[b, c]));
        assert_parent(world, c, Some(a));
        assert_events(
            world,
            &[ChildAdded {
                child: c,
                parent: a,
            }],
        );
        // Children component should be removed when it's empty.
        world.entity_mut(d).add_child(b).add_child(c);
        assert_children(world, a, None);
    }

    #[test]
    fn set_parent() {
        let world = &mut World::new();
        world.insert_resource(Events::<HierarchyEvent>::default());

        let [a, b, c] = core::array::from_fn(|_| world.spawn_empty().id());

        world.entity_mut(a).set_parent(b);

        assert_parent(world, a, Some(b));
        assert_children(world, b, Some(&[a]));
        assert_events(
            world,
            &[ChildAdded {
                child: a,
                parent: b,
            }],
        );

        world.entity_mut(a).set_parent(c);

        assert_parent(world, a, Some(c));
        assert_children(world, b, None);
        assert_children(world, c, Some(&[a]));
        assert_events(
            world,
            &[ChildMoved {
                child: a,
                previous_parent: b,
                new_parent: c,
            }],
        );
    }

    // regression test for https://github.com/bevyengine/bevy/pull/8346
    #[test]
    fn set_parent_of_orphan() {
        let world = &mut World::new();

        let [a, b, c] = core::array::from_fn(|_| world.spawn_empty().id());
        world.entity_mut(a).set_parent(b);
        assert_parent(world, a, Some(b));
        assert_children(world, b, Some(&[a]));

        world.entity_mut(b).despawn();
        world.entity_mut(a).set_parent(c);

        assert_parent(world, a, Some(c));
        assert_children(world, c, Some(&[a]));
    }

    #[test]
    fn remove_parent() {
        let world = &mut World::new();
        world.insert_resource(Events::<HierarchyEvent>::default());

        let [a, b, c] = core::array::from_fn(|_| world.spawn_empty().id());

        world.entity_mut(a).add_children(&[b, c]);
        world.entity_mut(b).remove_parent();

        assert_parent(world, b, None);
        assert_parent(world, c, Some(a));
        assert_children(world, a, Some(&[c]));
        omit_events(world, 2); // Omit ChildAdded events.
        assert_events(
            world,
            &[ChildRemoved {
                child: b,
                parent: a,
            }],
        );

        world.entity_mut(c).remove_parent();
        assert_parent(world, c, None);
        assert_children(world, a, None);
        assert_events(
            world,
            &[ChildRemoved {
                child: c,
                parent: a,
            }],
        );
    }

    #[allow(dead_code)]
    #[derive(Component)]
    struct C(u32);

    #[test]
    fn build_children() {
        let mut world = World::default();
        let mut queue = CommandQueue::default();
        let mut commands = Commands::new(&mut queue, &world);

        let parent = commands.spawn(C(1)).id();
        let mut children = Vec::new();
        commands.entity(parent).with_children(|parent| {
            children.extend([
                parent.spawn(C(2)).id(),
                parent.spawn(C(3)).id(),
                parent.spawn(C(4)).id(),
            ]);
        });

        queue.apply(&mut world);
        assert_eq!(
            world.get::<Children>(parent).unwrap().0.as_slice(),
            children.as_slice(),
        );
        assert_eq!(*world.get::<Parent>(children[0]).unwrap(), Parent(parent));
        assert_eq!(*world.get::<Parent>(children[1]).unwrap(), Parent(parent));

        assert_eq!(*world.get::<Parent>(children[0]).unwrap(), Parent(parent));
        assert_eq!(*world.get::<Parent>(children[1]).unwrap(), Parent(parent));
    }

    #[test]
    fn build_child() {
        let mut world = World::default();
        let mut queue = CommandQueue::default();
        let mut commands = Commands::new(&mut queue, &world);

        let parent = commands.spawn(C(1)).id();
        commands.entity(parent).with_child(C(2));

        queue.apply(&mut world);
        assert_eq!(world.get::<Children>(parent).unwrap().0.len(), 1);
    }

    #[test]
    fn push_and_insert_and_remove_children_commands() {
        let mut world = World::default();
        let entities = world
            .spawn_batch(vec![C(1), C(2), C(3), C(4), C(5)])
            .collect::<Vec<Entity>>();

        let mut queue = CommandQueue::default();
        {
            let mut commands = Commands::new(&mut queue, &world);
            commands.entity(entities[0]).add_children(&entities[1..3]);
        }
        queue.apply(&mut world);

        let parent = entities[0];
        let child1 = entities[1];
        let child2 = entities[2];
        let child3 = entities[3];
        let child4 = entities[4];

        let expected_children: SmallVec<[Entity; 8]> = smallvec![child1, child2];
        assert_eq!(
            world.get::<Children>(parent).unwrap().0.clone(),
            expected_children
        );
        assert_eq!(*world.get::<Parent>(child1).unwrap(), Parent(parent));
        assert_eq!(*world.get::<Parent>(child2).unwrap(), Parent(parent));

        assert_eq!(*world.get::<Parent>(child1).unwrap(), Parent(parent));
        assert_eq!(*world.get::<Parent>(child2).unwrap(), Parent(parent));

        {
            let mut commands = Commands::new(&mut queue, &world);
            commands.entity(parent).insert_children(1, &entities[3..]);
        }
        queue.apply(&mut world);

        let expected_children: SmallVec<[Entity; 8]> = smallvec![child1, child3, child4, child2];
        assert_eq!(
            world.get::<Children>(parent).unwrap().0.clone(),
            expected_children
        );
        assert_eq!(*world.get::<Parent>(child3).unwrap(), Parent(parent));
        assert_eq!(*world.get::<Parent>(child4).unwrap(), Parent(parent));

        let remove_children = [child1, child4];
        {
            let mut commands = Commands::new(&mut queue, &world);
            commands.entity(parent).remove_children(&remove_children);
        }
        queue.apply(&mut world);

        let expected_children: SmallVec<[Entity; 8]> = smallvec![child3, child2];
        assert_eq!(
            world.get::<Children>(parent).unwrap().0.clone(),
            expected_children
        );
        assert!(world.get::<Parent>(child1).is_none());
        assert!(world.get::<Parent>(child4).is_none());
    }

    #[test]
    fn push_and_clear_children_commands() {
        let mut world = World::default();
        let entities = world
            .spawn_batch(vec![C(1), C(2), C(3), C(4), C(5)])
            .collect::<Vec<Entity>>();

        let mut queue = CommandQueue::default();
        {
            let mut commands = Commands::new(&mut queue, &world);
            commands.entity(entities[0]).add_children(&entities[1..3]);
        }
        queue.apply(&mut world);

        let parent = entities[0];
        let child1 = entities[1];
        let child2 = entities[2];

        let expected_children: SmallVec<[Entity; 8]> = smallvec![child1, child2];
        assert_eq!(
            world.get::<Children>(parent).unwrap().0.clone(),
            expected_children
        );
        assert_eq!(*world.get::<Parent>(child1).unwrap(), Parent(parent));
        assert_eq!(*world.get::<Parent>(child2).unwrap(), Parent(parent));

        {
            let mut commands = Commands::new(&mut queue, &world);
            commands.entity(parent).clear_children();
        }
        queue.apply(&mut world);

        assert!(world.get::<Children>(parent).is_none());

        assert!(world.get::<Parent>(child1).is_none());
        assert!(world.get::<Parent>(child2).is_none());
    }

    #[test]
    fn push_and_replace_children_commands() {
        let mut world = World::default();
        let entities = world
            .spawn_batch(vec![C(1), C(2), C(3), C(4), C(5)])
            .collect::<Vec<Entity>>();

        let mut queue = CommandQueue::default();
        {
            let mut commands = Commands::new(&mut queue, &world);
            commands.entity(entities[0]).add_children(&entities[1..3]);
        }
        queue.apply(&mut world);

        let parent = entities[0];
        let child1 = entities[1];
        let child2 = entities[2];
        let child4 = entities[4];

        let expected_children: SmallVec<[Entity; 8]> = smallvec![child1, child2];
        assert_eq!(
            world.get::<Children>(parent).unwrap().0.clone(),
            expected_children
        );
        assert_eq!(*world.get::<Parent>(child1).unwrap(), Parent(parent));
        assert_eq!(*world.get::<Parent>(child2).unwrap(), Parent(parent));

        let replace_children = [child1, child4];
        {
            let mut commands = Commands::new(&mut queue, &world);
            commands.entity(parent).replace_children(&replace_children);
        }
        queue.apply(&mut world);

        let expected_children: SmallVec<[Entity; 8]> = smallvec![child1, child4];
        assert_eq!(
            world.get::<Children>(parent).unwrap().0.clone(),
            expected_children
        );
        assert_eq!(*world.get::<Parent>(child1).unwrap(), Parent(parent));
        assert_eq!(*world.get::<Parent>(child4).unwrap(), Parent(parent));
        assert!(world.get::<Parent>(child2).is_none());
    }

    #[test]
    fn push_and_insert_and_remove_children_world() {
        let mut world = World::default();
        let entities = world
            .spawn_batch(vec![C(1), C(2), C(3), C(4), C(5)])
            .collect::<Vec<Entity>>();

        world.entity_mut(entities[0]).add_children(&entities[1..3]);

        let parent = entities[0];
        let child1 = entities[1];
        let child2 = entities[2];
        let child3 = entities[3];
        let child4 = entities[4];

        let expected_children: SmallVec<[Entity; 8]> = smallvec![child1, child2];
        assert_eq!(
            world.get::<Children>(parent).unwrap().0.clone(),
            expected_children
        );
        assert_eq!(*world.get::<Parent>(child1).unwrap(), Parent(parent));
        assert_eq!(*world.get::<Parent>(child2).unwrap(), Parent(parent));

        world.entity_mut(parent).insert_children(1, &entities[3..]);
        let expected_children: SmallVec<[Entity; 8]> = smallvec![child1, child3, child4, child2];
        assert_eq!(
            world.get::<Children>(parent).unwrap().0.clone(),
            expected_children
        );
        assert_eq!(*world.get::<Parent>(child3).unwrap(), Parent(parent));
        assert_eq!(*world.get::<Parent>(child4).unwrap(), Parent(parent));

        let remove_children = [child1, child4];
        world.entity_mut(parent).remove_children(&remove_children);
        let expected_children: SmallVec<[Entity; 8]> = smallvec![child3, child2];
        assert_eq!(
            world.get::<Children>(parent).unwrap().0.clone(),
            expected_children
        );
        assert!(world.get::<Parent>(child1).is_none());
        assert!(world.get::<Parent>(child4).is_none());
    }

    #[test]
    fn push_and_insert_and_clear_children_world() {
        let mut world = World::default();
        let entities = world
            .spawn_batch(vec![C(1), C(2), C(3)])
            .collect::<Vec<Entity>>();

        world.entity_mut(entities[0]).add_children(&entities[1..3]);

        let parent = entities[0];
        let child1 = entities[1];
        let child2 = entities[2];

        let expected_children: SmallVec<[Entity; 8]> = smallvec![child1, child2];
        assert_eq!(
            world.get::<Children>(parent).unwrap().0.clone(),
            expected_children
        );
        assert_eq!(*world.get::<Parent>(child1).unwrap(), Parent(parent));
        assert_eq!(*world.get::<Parent>(child2).unwrap(), Parent(parent));

        world.entity_mut(parent).clear_children();
        assert!(world.get::<Children>(parent).is_none());
        assert!(world.get::<Parent>(child1).is_none());
        assert!(world.get::<Parent>(child2).is_none());
    }

    #[test]
    fn push_and_replace_children_world() {
        let mut world = World::default();
        let entities = world
            .spawn_batch(vec![C(1), C(2), C(3), C(4), C(5)])
            .collect::<Vec<Entity>>();

        world.entity_mut(entities[0]).add_children(&entities[1..3]);

        let parent = entities[0];
        let child1 = entities[1];
        let child2 = entities[2];
        let child3 = entities[3];
        let child4 = entities[4];

        let expected_children: SmallVec<[Entity; 8]> = smallvec![child1, child2];
        assert_eq!(
            world.get::<Children>(parent).unwrap().0.clone(),
            expected_children
        );
        assert_eq!(*world.get::<Parent>(child1).unwrap(), Parent(parent));
        assert_eq!(*world.get::<Parent>(child2).unwrap(), Parent(parent));

        world.entity_mut(parent).replace_children(&entities[2..]);
        let expected_children: SmallVec<[Entity; 8]> = smallvec![child2, child3, child4];
        assert_eq!(
            world.get::<Children>(parent).unwrap().0.clone(),
            expected_children
        );
        assert!(world.get::<Parent>(child1).is_none());
        assert_eq!(*world.get::<Parent>(child2).unwrap(), Parent(parent));
        assert_eq!(*world.get::<Parent>(child3).unwrap(), Parent(parent));
        assert_eq!(*world.get::<Parent>(child4).unwrap(), Parent(parent));
    }

    /// Tests what happens when all children are removed from a parent using world functions
    #[test]
    fn children_removed_when_empty_world() {
        let mut world = World::default();
        let entities = world
            .spawn_batch(vec![C(1), C(2), C(3)])
            .collect::<Vec<Entity>>();

        let parent1 = entities[0];
        let parent2 = entities[1];
        let child = entities[2];

        // add child into parent1
        world.entity_mut(parent1).add_children(&[child]);
        assert_eq!(
            world.get::<Children>(parent1).unwrap().0.as_slice(),
            &[child]
        );

        // move only child from parent1 with `add_children`
        world.entity_mut(parent2).add_children(&[child]);
        assert!(world.get::<Children>(parent1).is_none());

        // move only child from parent2 with `insert_children`
        world.entity_mut(parent1).insert_children(0, &[child]);
        assert!(world.get::<Children>(parent2).is_none());

        // remove only child from parent1 with `remove_children`
        world.entity_mut(parent1).remove_children(&[child]);
        assert!(world.get::<Children>(parent1).is_none());
    }

    /// Tests what happens when all children are removed form a parent using commands
    #[test]
    fn children_removed_when_empty_commands() {
        let mut world = World::default();
        let entities = world
            .spawn_batch(vec![C(1), C(2), C(3)])
            .collect::<Vec<Entity>>();

        let parent1 = entities[0];
        let parent2 = entities[1];
        let child = entities[2];

        let mut queue = CommandQueue::default();

        // add child into parent1
        {
            let mut commands = Commands::new(&mut queue, &world);
            commands.entity(parent1).add_children(&[child]);
            queue.apply(&mut world);
        }
        assert_eq!(
            world.get::<Children>(parent1).unwrap().0.as_slice(),
            &[child]
        );

        // move only child from parent1 with `add_children`
        {
            let mut commands = Commands::new(&mut queue, &world);
            commands.entity(parent2).add_children(&[child]);
            queue.apply(&mut world);
        }
        assert!(world.get::<Children>(parent1).is_none());

        // move only child from parent2 with `insert_children`
        {
            let mut commands = Commands::new(&mut queue, &world);
            commands.entity(parent1).insert_children(0, &[child]);
            queue.apply(&mut world);
        }
        assert!(world.get::<Children>(parent2).is_none());

        // move only child from parent1 with `add_child`
        {
            let mut commands = Commands::new(&mut queue, &world);
            commands.entity(parent2).add_child(child);
            queue.apply(&mut world);
        }
        assert!(world.get::<Children>(parent1).is_none());

        // remove only child from parent2 with `remove_children`
        {
            let mut commands = Commands::new(&mut queue, &world);
            commands.entity(parent2).remove_children(&[child]);
            queue.apply(&mut world);
        }
        assert!(world.get::<Children>(parent2).is_none());
    }

    #[test]
    fn regression_add_children_same_archetype() {
        let mut world = World::new();
        let child = world.spawn_empty().id();
        world.spawn_empty().add_children(&[child]);
    }

    #[test]
    fn add_children_idempotent() {
        let mut world = World::new();
        let child = world.spawn_empty().id();
        let parent = world
            .spawn_empty()
            .add_children(&[child])
            .add_children(&[child])
            .id();

        let mut query = world.query::<&Children>();
        let children = query.get(&world, parent).unwrap();
        assert_eq!(**children, [child]);
    }

    #[test]
    fn add_children_does_not_insert_empty_children() {
        let mut world = World::new();
        let parent = world.spawn_empty().add_children(&[]).id();

        let mut query = world.query::<&Children>();
        let children = query.get(&world, parent);
        assert!(children.is_err());
    }

    #[test]
    fn with_child() {
        let world = &mut World::new();
        world.insert_resource(Events::<HierarchyEvent>::default());

        let a = world.spawn_empty().id();
        let b = ();
        let c = ();
        let d = ();

        world.entity_mut(a).with_child(b);

        assert_num_children(world, a, 1);

        world.entity_mut(a).with_child(c).with_child(d);

        assert_num_children(world, a, 3);
    }
}
