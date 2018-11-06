//! Utility methods and constructors for Lua classes

use std::convert::From;
use std::sync::Arc;
use std::marker::PhantomData;

use rlua::{self, AnyUserData, Function, Lua, MetaMethod, Table, ToLua, UserData, UserDataMethods,
           Value};

use super::object::{self, Object, ObjectStateType};
use super::property::Property;

pub type Checker<S> = Arc<Fn(Object<S>) -> bool + Send + Sync>;

#[derive(Clone, Debug)]
pub struct Class<'lua, S: ObjectStateType> {
    class: AnyUserData<'lua>,
    kind: PhantomData<S>,
}

impl<'lua, S: ObjectStateType> From<AnyUserData<'lua>> for Class<'lua, S> {
    fn from(class: AnyUserData<'lua>) -> Self {
        Class { class, kind: PhantomData }
    }
}

#[derive(Clone)]
pub struct ClassState<S: ObjectStateType> {
    // NOTE That this is missing fields from the C version.
    // They are stored in the meta table instead, to not have unsafety.
    // They are fetchable using getters.
    checker: Option<Checker<S>>,
    instances: u32
}

pub struct ClassBuilder<'lua, S: ObjectStateType> {
    lua: &'lua Lua,
    class: Class<'lua, S>
}

impl<'lua, S: ObjectStateType> ClassBuilder<'lua, S> {
    pub fn method(self, name: String, meth: Function) -> rlua::Result<Self> {
        let table = self.class.class.get_user_value::<Table>()?;
        let meta = table.get_metatable().expect("Class had no meta table!");
        meta.set(name, meth)?;
        Ok(self)
    }

    pub fn property(self, prop: Property<'lua>) -> rlua::Result<Self> {
        let table = self.class.class.get_user_value::<Table>()?;
        let properties = table.get::<_, Table>("properties")?;
        let length = properties.len().unwrap_or(0) + 1;
        properties.set(length, prop)?;
        Ok(self)
    }

    // TODO remove, do right
    #[allow(dead_code)]
    pub fn dummy_property(self, key: String, val: Value<'lua>) -> rlua::Result<Self> {
        let table = self.class.class.get_user_value::<Table>()?;
        let meta = table.get_metatable().expect("Class had no meta table!");
        meta.set(key, val)?;
        Ok(self)
    }

    pub fn save_class(self, name: &str) -> rlua::Result<Self> {
        self.lua.globals().set(name, self.class.class.clone())?;
        Ok(self)
    }

    pub fn build(self) -> rlua::Result<Class<'lua, S>> {
        Ok(self.class)
    }
}

impl<'lua, S: ObjectStateType> ToLua<'lua> for Class<'lua, S> {
    fn to_lua(self, lua: &'lua Lua) -> rlua::Result<Value<'lua>> {
        self.class.to_lua(lua)
    }
}

impl<S: ObjectStateType> Default for ClassState<S> {
    fn default() -> Self {
        ClassState { checker: Option::default(),
                     instances: 0 }
    }
}

impl<S: ObjectStateType> UserData for ClassState<S> {
    fn add_methods(methods: &mut UserDataMethods<Self>) {
        methods.add_meta_function(MetaMethod::Index, class_index);
        // TODO Class new index?
        methods.add_meta_function(MetaMethod::NewIndex, object::default_newindex::<S>);
        fn call<'lua>(lua: &'lua Lua,
                      (class, args): (AnyUserData<'lua>, rlua::MultiValue<'lua>))
                      -> rlua::Result<Value<'lua>> {
            match class_index(lua, (class, "__call".to_lua(lua)?))? {
                Value::Function(function) => function.call(args),
                v => Ok(v)
            }
        }
        methods.add_meta_function(MetaMethod::Call, call);
        methods.add_meta_function(MetaMethod::ToString, |_, class: AnyUserData| {
            let table = class.get_user_value::<Table>()?;
            table.get::<_, String>("name")
        });
    }
}

impl<'lua, S: ObjectStateType> Class<'lua, S> {
    pub fn builder(lua: &'lua Lua,
                   name: &str,
                   checker: Option<Checker<S>>)
                   -> rlua::Result<ClassBuilder<'lua, S>> {
        let mut class = ClassState::default();
        class.checker = checker;
        let user_data = lua.create_userdata(class)?;
        let table = lua.create_table()?;
        // Store in not meta table so we can't index it
        table.set("name", name)?;
        table.set("properties", Vec::<Property>::new().to_lua(lua)?)?;
        let meta = lua.create_table()?;
        meta.set("signals", lua.create_table()?)?;
        meta.set("set_index_miss_handler",
                  lua.create_function(set_index_miss_handler)?
                     .bind(user_data.clone())?)?;
        meta.set("set_newindex_miss_handler",
                  lua.create_function(set_newindex_miss_handler)?
                     .bind(user_data.clone())?)?;
        meta.set("__index", meta.clone())?;
        table.set_metatable(Some(meta.clone()));
        user_data.set_user_value(table)?;
        Ok(ClassBuilder { lua: lua,
                          class: Class { class: user_data, kind: PhantomData } })
    }

    pub fn checker(&self) -> rlua::Result<Option<Checker<S>>> {
        self.class.borrow::<ClassState<S>>()
            .map(|class| class.checker.clone())
    }
}

fn set_index_miss_handler<'lua>(_: &'lua Lua,
                                (class, func): (AnyUserData, Function))
                                -> rlua::Result<()> {
    let table = class.get_user_value::<Table>()?;
    let meta = table.get_metatable().expect("Object had no metatable");
    meta.set("__index_miss_handler", func)?;
    Ok(())
}

fn set_newindex_miss_handler<'lua>(_: &'lua Lua,
                                   (class, func): (AnyUserData, Function))
                                   -> rlua::Result<()> {
    let table = class.get_user_value::<Table>()?;
    let meta = table.get_metatable().expect("Object had no metatable");
    meta.set("__newindex_miss_handler", func)?;
    Ok(())
}

pub fn class_setup<'lua, S: ObjectStateType>(lua: &'lua Lua,
                                   name: &str)
                                   -> rlua::Result<Class<'lua, S>> {
    let class = lua.globals()
                   .get::<_, AnyUserData>(name)
                   .expect("Class was not set! Did you call init?");
    assert!(class.is::<ClassState<S>>()?, "This user data was not a class!");
    Ok(Class { class, kind: PhantomData })
}

fn class_index<'lua>(_: &'lua Lua,
                     (class, index): (AnyUserData<'lua>, Value<'lua>))
                     -> rlua::Result<Value<'lua>> {
    let table = class.get_user_value::<Table>()?;
    let meta = table.get_metatable().expect("class had no meta table");
    match meta.raw_get("__index")? {
        Value::Function(function) => function.call((class, index)),
        Value::Table(table) => table.get(index),
        _ => panic!("Unexpected value in index")
    }
}
