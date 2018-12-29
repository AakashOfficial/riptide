//! The Riptide runtime.
use crate::builtins;
use crate::exceptions::Exception;
use crate::modules;
use crate::stdlib;
use crate::string::RipString;
use crate::syntax;
use crate::syntax::ast::*;
use crate::syntax::source::*;
use crate::table::Table;
use crate::value::*;
use log::*;
use std::rc::Rc;

/// A native function that can be called by managed code.
pub type ForeignFunction = fn(&mut Runtime, &[Value]) -> Result<Value, Exception>;

/// A function evaluation scope.
///
/// A scope encompasses the _environment_ in which functions are evaluated. Scopes are hierarchial, and contain a
/// reference to the enclosing, or parent, scope.
#[derive(Clone, Debug, Default)]
pub struct Scope {
    name: Option<String>,

    /// The function reference to invoke.
    function: Value,

    args: Vec<Value>,

    bindings: Rc<Table>,

    pub(crate) parent: Option<Rc<Scope>>,
}

impl Scope {
    /// Get the name of this scope, if available.
    pub fn name(&self) -> &str {
        self.name.as_ref().map(|s| &s as &str).unwrap_or("<unknown>")
    }

    /// Get the arguments passed in to the current scope.
    pub fn args(&self) -> &[Value] {
        &self.args
    }

    /// Lookup a variable name in the current scope.
    pub fn get(&self, name: impl AsRef<[u8]>) -> Value {
        let name = name.as_ref();

        match self.bindings.get(name) {
            Value::Nil => match self.parent.as_ref() {
                Some(parent) => parent.get(name),
                None => Value::Nil,
            },
            value => value,
        }
    }

    /// Set a variable value in the current scope.
    pub fn set(&self, name: impl Into<RipString>, value: impl Into<Value>) {
        self.bindings.set(name, value);
    }
}

/// Configure a runtime.
pub struct RuntimeBuilder {
    module_loaders: Vec<Value>,
    globals: Table,
}

impl Default for RuntimeBuilder {
    fn default() -> Self {
        Self::new().module_loader(modules::relative_loader).module_loader(modules::system_loader).with_stdlib()
    }
}

impl RuntimeBuilder {
    pub fn new() -> Self {
        Self {
            module_loaders: Vec::new(),
            globals: table! {
                "require" => Value::ForeignFunction(modules::require),
                "args" => Value::ForeignFunction(builtins::args),
                "backtrace" => Value::ForeignFunction(builtins::backtrace),
                "call" => Value::ForeignFunction(builtins::call),
                "catch" => Value::ForeignFunction(builtins::catch),
                "def" => Value::ForeignFunction(builtins::def),
                "defglobal" => Value::ForeignFunction(builtins::defglobal),
                "list" => Value::ForeignFunction(builtins::list),
                "nil" => Value::ForeignFunction(builtins::nil),
                "throw" => Value::ForeignFunction(builtins::throw),
                "typeof" => Value::ForeignFunction(builtins::type_of),
                "modules" => Value::from(table! {
                    "loaded" => Value::from(table!()),
                    "loaders" => Value::Nil,
                }),
            },
        }
    }

    /// Register a module loader.
    pub fn module_loader(mut self, loader: ForeignFunction) -> Self {
        self.module_loaders.push(loader.into());
        self
    }

    pub fn with_stdlib(self) -> Self {
        self.module_loader(stdlib::stdlib_loader)
    }

    pub fn build(self) -> Runtime {
        self.globals.get("modules").as_table().unwrap().set("loaders", Value::List(self.module_loaders));

        let mut runtime = Runtime {
            globals: Rc::new(self.globals),
            stack: Vec::new(),
            is_exiting: false,
            exit_code: 0,
        };

        runtime.init();

        runtime
    }
}

/// Holds all of the state of a Riptide runtime.
pub struct Runtime {
    /// Table where global values are stored that are not on the stack.
    globals: Rc<Table>,

    /// Current call stack.
    ///
    /// This is exposed to the rest of the crate to support the `backtrace` function.
    pub(crate) stack: Vec<Rc<Scope>>,

    /// If application code inside the runtime requests the runtime to exit, this is set to true.
    is_exiting: bool,

    /// The runtime exit code to return after it shuts down.
    exit_code: i32,
}

impl Default for Runtime {
    fn default() -> Self {
        RuntimeBuilder::default().build()
    }
}

impl Runtime {
    /// Initialize the runtime environment.
    fn init(&mut self) {
        self.stack.push(Rc::new(Scope {
            name: Some(String::from("<global scope>")),
            function: Value::Nil,
            args: Vec::new(),
            bindings: self.globals.clone(),
            parent: None,
        }));
        self.globals.set("GLOBALS", self.globals.clone());

        self.execute(None, include_str!("init.rip")).expect("error in runtime initialization");
    }

    pub fn builder() -> RuntimeBuilder {
        RuntimeBuilder::new()
    }

    pub fn exit_code(&self) -> i32 {
        self.exit_code
    }

    pub fn exit_requested(&self) -> bool {
        self.is_exiting
    }

    /// Request the runtime to exit with the given exit code.
    ///
    /// The runtime will exit gracefully.
    pub fn exit(&mut self, code: i32) {
        debug!("runtime exit requested with exit code {}", code);
        self.exit_code = code;
        self.is_exiting = true;
    }

    /// Get the table that holds all global variables.
    pub fn globals(&self) -> &Table {
        &self.globals
    }

    /// Get a reference to the current scope.
    pub fn scope(&self) -> &Scope {
        self.stack.last().unwrap()
    }

    /// Lookup a variable name in the current scope.
    pub fn get(&self, name: impl AsRef<[u8]>) -> Value {
        let name = name.as_ref();

        if let Some(scope) = self.stack.last() {
            match scope.get(name) {
                Value::Nil => {},
                value => return value,
            }
        }

        self.globals.get(name)
    }

    fn get_path(&self, path: &VariablePath) -> Value {
        let mut result = self.get(&path.0[0]);

        if path.0.len() > 1 {
            for part in &path.0[1..] {
                result = result.get(part);
            }
        }

        result
    }

    /// Execute the given script within this runtime.
    ///
    /// The script will be executed inside the context of the module with the given name. If no module name is given, an
    /// anonymous module will be created for the file.
    ///
    /// If a compilation error occurs with the given file, an exception will be returned.
    pub fn execute(&mut self, module: Option<&str>, file: impl Into<SourceFile>) -> Result<Value, Exception> {
        let file = file.into();
        let file_name = file.name().to_string();

        let block = match syntax::parse(file) {
            Ok(block) => block,
            Err(e) => throw!("error parsing: {}", e),
        };

        let closure = Closure {
            block: block,
            scope: None,
        };

        let module_scope = match module {
            Some(name) => self.get_module_by_name(name),
            None => Rc::new(table!()),
        };

        // HACK...
        self.stack.push(Rc::new(Scope {
            name: Some(file_name),
            function: Value::Nil,
            args: Vec::new(),
            bindings: module_scope,
            parent: self.stack.last().cloned(),
        }));

        let result = self.invoke_closure(&closure, &[]);

        self.stack.pop();

        result
    }

    /// Invoke the given value as a function with the given arguments.
    pub fn invoke(&mut self, value: &Value, args: &[Value]) -> Result<Value, Exception> {
        self.stack.push(Rc::new(Scope {
            name: Some(String::from("<closure>")),
            function: value.clone(),
            args: args.to_vec(),
            bindings: Rc::new(table!()),
            parent: self.stack.last().cloned(),
        }));

        let result = self.do_invoke();

        self.stack.pop();

        result
    }

    /// Get a module scope table by the module's name. If the module table does
    /// not already exist, it will be created.
    fn get_module_by_name(&self, name: &str) -> Rc<Table> {
        let loaded = self.globals.get("modules").get("loaded");

        if loaded.get(name).is_nil() {
            loaded.as_table().unwrap().set(name, table!());
        }

        loaded.get(name).as_table().unwrap().clone()
    }

    // /// Open a new scope.
    // fn begin_scope(&mut self, args: Vec<Value>) {
    //     let scope = Scope {
    //         name: None,
    //         args: args,
    //         bindings: Rc::new(table!()),
    //         parent: self.stack.last().cloned().map(Box::new),
    //     };
    //     self.stack.push(scope);
    // }

    // /// Close the current scope.
    // fn end_scope(&mut self) {
    //     self.stack.pop();
    // }

    /// Invoke the function at the top of the stack.
    fn do_invoke(&mut self) -> Result<Value, Exception> {
        let function = self.stack.last().unwrap().function.clone();
        let args = self.stack.last().unwrap().args.to_vec();

        match function {
            Value::Block(closure) => {
                // self.begin_scope(scope.args.to_vec());

                let result = self.invoke_closure(&*closure, &args);

                // Stack must be unwound regardless of exceptions.
                // self.end_scope();

                result
            }
            Value::ForeignFunction(function) => (function)(self, &args),
            value => throw!("cannot invoke '{:?}' as a function", value),
        }
    }

    /// Invoke a block with an array of arguments.
    fn invoke_closure(&mut self, closure: &Closure, _: &[Value]) -> Result<Value, Exception> {
        let mut last_return_value = Value::Nil;

        // Evaluate each statement in order.
        for statement in closure.block.statements.iter() {
            match self.evaluate_pipeline(&statement) {
                Ok(return_value) => last_return_value = return_value,
                Err(exception) => {
                    // Exception thrown; abort and unwind stack.
                    return Err(exception);
                }
            }
        }

        Ok(last_return_value)
    }

    fn evaluate_pipeline(&mut self, pipeline: &Pipeline) -> Result<Value, Exception> {
        // If there's only one call in the pipeline, we don't need to fork and can just execute the function by itself.
        if pipeline.items.len() == 1 {
            self.evaluate_call(pipeline.items[0].clone())
        } else {
            warn!("parallel pipelines not implemented!");
            Ok(Value::Nil)
        }
    }

    fn evaluate_call(&mut self, call: Call) -> Result<Value, Exception> {
        let (function, args) = match call {
            Call::Named(path, args) => (self.get_path(&path), args),
            Call::Unnamed(function, args) => (
                {
                    let mut function = self.evaluate_expr(*function)?;

                    // If the function is a string, resolve binding names first before we try to eval the item as a function.
                    if let Some(value) = function.as_string().map(|name| self.get(name)) {
                        function = value;
                    }

                    function
                },
                args,
            ),
        };

        let mut arg_values = Vec::with_capacity(args.len());
        for expr in args {
            arg_values.push(self.evaluate_expr(expr)?);
        }

        self.invoke(&function, &arg_values)
    }

    fn evaluate_expr(&mut self, expr: Expr) -> Result<Value, Exception> {
        match expr {
            Expr::Number(number) => Ok(Value::Number(number)),
            Expr::String(string) => Ok(Value::from(string)),
            // TODO: Handle expands
            Expr::InterpolatedString(_) => {
                warn!("string interpolation not yet supported");
                Ok(Value::Nil)
            },
            Expr::Substitution(substitution) => self.evaluate_substitution(substitution),
            Expr::Block(block) => Ok(Value::from(Closure {
                block: block,
                scope: Some(self.scope().clone()),
            })),
            Expr::Pipeline(ref pipeline) => self.evaluate_pipeline(pipeline),
        }
    }

    fn evaluate_substitution(&mut self, substitution: Substitution) -> Result<Value, Exception> {
        match substitution {
            Substitution::Variable(path) => Ok(self.get_path(&path)),
            Substitution::Pipeline(ref pipeline) => self.evaluate_pipeline(pipeline),
            _ => unimplemented!(),
        }
    }
}
