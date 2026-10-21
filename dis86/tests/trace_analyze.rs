// Tests for the Hydra trace reader (issue #97): the parser has to accept
// what the C writer produces, reject what it does not, and turn the events
// into the function-level picture a reverse engineer works from.
use dis86::segoff::SegOff;
use dis86::trace::{analyze, parse_trace, TraceEvent};

// The trace the parity fixture produces (parity/build.sh, Gate 3). The
// addresses are absolute guest ones; the code load is 0001.
const PARITY_TRACE: &str = "\
# hydra trace v1
# code_load=0001
dispatch enter cs=0001 ip=0000 name=F_parity_main
native_call from=0001:0000 to=0001:0185 name=F_parity_leaf nargs=0
dispatch leave cs=0001 ip=0000 name=F_parity_main result=call_near
call from=0001:ff00 to=0001:0185 name=F_parity_leaf
dispatch enter cs=0001 ip=0185 name=F_parity_leaf
dispatch leave cs=0001 ip=0185 name=F_parity_leaf result=ret_near
ret from=0001:0185 to=0001:ff00 name=F_parity_leaf
native_ret from=0001:0000 to=0001:0185 dx=0000 ax=117a
ret from=0001:ff00 to=0001:0189
";

#[test]
fn parses_the_events_of_a_recorded_run() {
  let events = parse_trace(PARITY_TRACE).expect("the trace parses");
  assert_eq!(events.len(), 9);

  assert!(matches!(&events[0], TraceEvent::Dispatch { enter: true, addr: Some(a), name: Some(n), .. }
    if *a == SegOff::new(0, 0x0000) && n == "F_parity_main"));

  assert!(matches!(&events[1], TraceEvent::NativeCall { from: Some(f), to: Some(t), name: Some(n), nargs: 0, args }
    if *f == SegOff::new(0, 0x0000) && *t == SegOff::new(0, 0x0185)
       && n == "F_parity_leaf" && args.is_empty()));

  assert!(matches!(&events[7], TraceEvent::NativeRet { to: Some(t), dx: 0x0000, ax: 0x117a, .. }
    if *t == SegOff::new(0, 0x0185)));
}

#[test]
fn maps_the_guest_addresses_into_the_image_space() {
  let events = parse_trace(PARITY_TRACE).expect("the trace parses");

  // The code load is subtracted: the fixture's image-relative addresses are
  // the ones a configuration table uses.
  let mut seen = vec![];
  for event in &events {
    let addr = match event {
      TraceEvent::Dispatch { addr, .. } => *addr,
      TraceEvent::NativeCall { to, .. } => *to,
      TraceEvent::NativeRet { to, .. } => *to,
      _ => continue,
    };
    if let Some(addr) = addr { seen.push(addr); }
  }
  assert!(seen.contains(&SegOff::new(0, 0x0000)));
  assert!(seen.contains(&SegOff::new(0, 0x0185)));
}

#[test]
fn names_the_function_a_return_event_left() {
  // A function that only ever shows up in a return event still gets the
  // name the trace carries.
  let trace = "\
# hydra trace v1
# code_load=0001
ret from=0001:0200 to=0001:0180 name=F_only_returned
";
  let events = parse_trace(trace).expect("the trace parses");
  let a = analyze(&events);

  let f = a.functions.get(&SegOff::new(0, 0x0200)).expect("the function");
  assert_eq!(f.name.as_deref(), Some("F_only_returned"));
}

#[test]
fn drops_the_magic_return_addresses() {
  let events = parse_trace(PARITY_TRACE).expect("the trace parses");

  // 0xff00+exec_id is the window the core resumes a suspended hook at, and
  // 0xffff:exec_id is the far one. Neither is a place a function lives.
  for event in &events {
    let addrs = match event {
      TraceEvent::Call { from, to, .. } => (*from, *to),
      TraceEvent::Ret { from, to, .. } => (*from, *to),
      _ => continue,
    };
    for addr in [addrs.0, addrs.1].into_iter().flatten() {
      assert_ne!(addr, SegOff::new(0, 0xff00));
      assert_ne!(addr.off.0, 0xffff);
    }
  }
}

#[test]
fn builds_the_function_level_picture() {
  let events = parse_trace(PARITY_TRACE).expect("the trace parses");
  let a = analyze(&events);

  assert_eq!(a.events, 9);
  // The dispatches are the `enter` events: one per run of a hook, so the
  // number agrees with the machine's dispatch count instead of doubling it.
  assert_eq!(a.dispatches, 2);
  assert_eq!(a.calls, 1);
  assert_eq!(a.returns, 2);
  assert_eq!(a.native_calls, 1);
  assert_eq!(a.native_returns, 1);
  assert_eq!(a.steps, 0);
  assert_eq!(a.interrupts, 0);

  // Only the function entries are functions: the return addresses in the
  // caller are call sites, not entries.
  assert_eq!(a.functions.len(), 2);

  let main = a.functions.get(&SegOff::new(0, 0x0000)).expect("the entry function");
  assert_eq!(main.name.as_deref(), Some("F_parity_main"));
  assert_eq!(main.dispatches, 1);
  assert_eq!(main.returns, 1);
  assert!(main.callees.contains(&SegOff::new(0, 0x0185)));

  let leaf = a.functions.get(&SegOff::new(0, 0x0185)).expect("the leaf");
  assert_eq!(leaf.name.as_deref(), Some("F_parity_leaf"));
  assert_eq!(leaf.observed_args, Some(0));
  assert_eq!(leaf.results, vec![(0x0000, 0x117a)]);

  // Both ends of the call seam are known, so the graph can be read either
  // way.
  assert!(leaf.callers.contains(&SegOff::new(0, 0x0000)));
}

#[test]
fn rejects_a_trace_without_a_version_header() {
  let err = parse_trace("dispatch enter cs=0001 ip=0000\n").unwrap_err();
  assert!(err.contains("hydra trace v1"), "unexpected error: {}", err);
}

#[test]
fn rejects_an_unknown_event() {
  let err = parse_trace("# hydra trace v1\nwhat is this\n").unwrap_err();
  assert!(err.contains("unknown event"), "unexpected error: {}", err);
}

#[test]
fn rejects_a_malformed_field() {
  let err = parse_trace("# hydra trace v1\nstep cs=zz ip=0000\n").unwrap_err();
  assert!(err.contains("bad field"), "unexpected error: {}", err);

  let err = parse_trace("# hydra trace v1\nstep cs=0001\n").unwrap_err();
  assert!(err.contains("missing field"), "unexpected error: {}", err);
}

#[test]
fn reports_disagreeing_argument_counts() {
  let trace = "\
# hydra trace v1
# code_load=0001
native_call from=0001:0000 to=0001:0185 nargs=1 arg0=1234
native_call from=0001:0000 to=0001:0185 nargs=2 arg0=1234 arg1=5678
";
  let events = parse_trace(trace).expect("the trace parses");
  let a = analyze(&events);

  let leaf = a.functions.get(&SegOff::new(0, 0x0185)).expect("the leaf");
  assert!(leaf.args_conflict);
  // The highest count is the one reported: a signature has to hold every
  // word the callee may read.
  assert_eq!(leaf.observed_args, Some(2));
}

#[test]
fn does_not_invent_a_function_for_a_return_inside_one() {
  // A return the guest executed itself leaves the `from` of the event at
  // the return instruction, inside the function. That address is not a
  // function entry, so it must not become one: the discovery suggestions
  // would offer a function that does not exist.
  let trace = "\
# hydra trace v1
# code_load=0001
dispatch enter cs=0001 ip=013e name=F_myfunc
ret from=0001:0190 to=0001:0020
";
  let events = parse_trace(trace).expect("the trace parses");
  let a = analyze(&events);

  assert_eq!(a.returns, 1);
  assert!(!a.functions.contains_key(&SegOff::new(0, 0x0190)),
          "a mid-function address must not become a function");
  assert_eq!(a.functions.len(), 1);
}

#[test]
fn counts_the_hook_dispatches_and_not_the_dispatch_events() {
  // Every hook run writes an `enter` and a `leave` event. The dispatch
  // count is the number of hook runs, so it agrees with the machine's own
  // count rather than doubling it.
  let trace = "\
# hydra trace v1
# code_load=0001
dispatch enter cs=0001 ip=013e name=F_myfunc
dispatch leave cs=0001 ip=013e name=F_myfunc result=resume
dispatch enter cs=0001 ip=013e name=F_myfunc
dispatch leave cs=0001 ip=013e name=F_myfunc result=resume
";
  let events = parse_trace(trace).expect("the trace parses");
  let a = analyze(&events);

  assert_eq!(a.dispatches, 2);
  let f = a.functions.get(&SegOff::new(0, 0x013e)).expect("the function");
  assert_eq!(f.dispatches, 2);
  assert_eq!(f.returns, 2);
}

#[test]
fn rejects_an_interrupt_number_that_is_not_a_byte() {
  let err = parse_trace("# hydra trace v1\nint num=1ff ah=4c\n").unwrap_err();
  assert!(err.contains("not a byte"), "unexpected error: {}", err);

  let err = parse_trace("# hydra trace v1\nint num=21 ah=100\n").unwrap_err();
  assert!(err.contains("not a byte"), "unexpected error: {}", err);
}
