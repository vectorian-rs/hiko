
Name: Hiko

Project goal:

Build a new **strict, statically typed, ML-family scripting language** with an **SML / Haskell-like feel**, implemented in **Rust**, with a **bytecode VM/runtime** suitable for embedding and scripting.

The language should follow the **core language spirit of Standard ML** much more closely than the module layer. I want the implementation to take strong guidance from the **core of the SML standard / Definition of Standard ML**, especially the first part covering the **core language**, while **explicitly excluding** the advanced module machinery for now.

Target semantic position:

* **strict / call-by-value**
* **static typing**
* **Hindley–Milner style inference**
* **algebraic data types**
* **pattern matching**
* **lexical scoping**
* **closures**
* **recursion**
* **tail-call optimization if feasible**

Important scope boundary:

Use the **core SML language** as the reference model, but **do not** implement the full SML module system initially.

Explicitly in scope from the SML core direction:

* value bindings
* function bindings
* anonymous functions
* let-expressions
* tuples
* lists
* algebraic datatypes
* case analysis / pattern matching
* recursive definitions
* polymorphic let where appropriate
* basic built-in types
* exhaustiveness/usefulness checking in pattern matching, at least in a practical limited form

Explicitly out of scope for the first implementation:

* signatures
* structures
* functors
* opaque ascription / sealing
* sharing constraints
* full Basis Library compatibility
* advanced module elaboration machinery

High-level design constraints:

* Runtime implemented in Rust
* Language is for scripting, so startup time, simplicity, and embeddability matter
* Prefer a **small, coherent core** over a fully standard-compliant SML implementation
* Syntax can be inspired by SML / Haskell, but the **semantics should track core SML more than Haskell**
* The implementation should be bootstrapped in pragmatic stages, not designed as a giant speculative architecture

What I want from you:
Produce a **concrete bootstrap plan** for the project, with emphasis on implementation order, semantics, and code structure. Be opinionated and practical.

A key requirement:
When proposing the language core, explicitly distinguish:

1. **what should match core SML closely**
2. **what can be simplified for a scripting-oriented first version**
3. **what should be deferred from the SML standard**

Deliverables:

1. A recommended **MVP language spec**

   * exact initial syntax/features
   * what is included in v0
   * what is explicitly deferred
   * which parts are intentionally aligned with **core SML**

2. A recommended **compiler/runtime architecture**

   * lexer/parser
   * AST
   * type representation
   * inference/checking strategy
   * lowering / IR
   * bytecode format
   * VM design
   * value representation
   * environment / closure representation
   * error reporting strategy

3. A **phased implementation roadmap**

   * phase 0: repository setup and crate layout
   * phase 1: parser + AST for the core language
   * phase 2: HM type inference for a minimal SML-like core
   * phase 3: bytecode compiler + VM
   * phase 4: ADTs + pattern matching
   * phase 5: imports / file modules / REPL / stdlib basics
   * for each phase, define:

     * target scope
     * success criteria
     * example tests
     * major risks

4. A proposed **Rust workspace layout**
   Example direction:

   * hiko/parser
   * hiko/ast
   * hiko/types
   * hiko/infer
   * hiko/bytecode
   * hiko/vm
   * hiko/cli
   * hiko/repl
     

5. A recommended **initial instruction set** for the VM
   Include a compact but usable bytecode design for:

   * constants
   * locals
   * closures
   * calls
   * returns
   * jumps
   * tuple/list/ADT construction
   * match/tag branching
   * primitive ops

6. A recommended **runtime representation**
   Explain how to represent:

   * ints, bools, strings
   * closures
   * tuples
   * ADT values
   * lists
   * environments / stack frames
   * heap allocation / GC strategy
     Be pragmatic about Rust ergonomics.

7. A recommended **type inference strategy**

   * HM / Algorithm W or close variant
   * type variables
   * substitutions / unification
   * polymorphic let-generalization
   * treatment of ADTs
   * exhaustiveness checking strategy for pattern matching
     Start simple if needed, but be explicit.

8. A clear **MVP syntax sketch**
   Include example code for:

   * val/let bindings
   * function definitions
   * lambdas
   * datatype declarations
   * case / pattern matching
   * recursion
   * file/module import
     Keep the syntax small and internally consistent.

9. A **testing strategy**

   * parser golden tests
   * type inference tests
   * VM execution tests
   * end-to-end language tests
   * regression tests for pattern matching and recursion

10. A short section called **“What not to build yet”**
    Explicitly defer:

* full SML modules
* functors
* signatures
* opaque ascription
* type classes / traits
* laziness
* effect systems
* optimizing JIT
* sophisticated package manager
  unless you think one very small exception is justified

Technical preferences:

* Prefer a **bytecode VM** over a JIT for the first version
* Prefer a **simple internal IR** only if it earns its keep
* Prefer a **small initial stdlib**
* Prefer **good diagnostics** over clever implementation tricks
* Prefer implementation choices that are idiomatic in Rust and tractable for one engineer
* If GC is needed, recommend a realistic Rust approach
* Tail-call optimization is desirable but can be staged
* Exhaustiveness checking for match is important, but can start with a limited useful version

Language positioning:
This should feel like:

* an ML-family scripting language
* practical, small, embeddable
* more like “strict typed functional scripting on a Rust VM”
* semantically closer to **core SML**
* not like a full clone of Standard ML with the entire module system
* not like lazy Haskell

Important instruction:
Please reference the **core language of Standard ML** as a design anchor. Treat the **module language as intentionally deferred**. Where SML core semantics are too heavy for v0, propose the smallest simplification that preserves the overall character of the language.

Also include:

* 1 recommended file extension
* 1 recommended CLI command name

Output format:
Write the result as a **technical project bootstrap document** with the following sections:

* Project summary
* Core SML alignment
* MVP language
* Architecture
* Roadmap
* Workspace layout
* VM design
* Type system plan
* Syntax sketch
* Testing plan
* Deferred features
* Final recommendation

Be concrete. Avoid vague advice. When there are tradeoffs, choose one and justify it.

