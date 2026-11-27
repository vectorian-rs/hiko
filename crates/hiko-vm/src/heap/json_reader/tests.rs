use super::*;

#[test]
fn close_releases_slots_immediately_and_refunds_payload_once() {
    let mut heap = Heap::new();
    heap.set_max_bytes(2_000_000);
    heap.set_max_host_resources(32);
    heap.set_host_resource_limits(HashMap::from([("json_reader".into(), 32)]));
    let bytes = heap.memory_budget().unwrap().clone();
    let input: Arc<str> = "[1,2]".into();
    for i in 1..=64 {
        let value = heap.new_json_reader(input.clone()).unwrap();
        assert_eq!(heap.host_resource_count(), 1);
        assert!(heap.current_bytes > i * Heap::json_handle_bytes());
        heap.close_json_reader(value).unwrap();
        assert_eq!(heap.host_resource_count(), 0);
        assert_eq!(
            heap.host_resource_count_by_kind(HostHandleKind::JsonReader),
            0
        );
        assert_eq!(heap.current_bytes, i * Heap::json_handle_bytes());
        assert_eq!(bytes.used(), heap.current_bytes as u64);
        let before = bytes.used();
        heap.close_json_reader(value).unwrap();
        assert_eq!(bytes.used(), before);
    }
    assert_eq!(heap.live_count(), 64); // Closed handles are not live resources.
    assert_eq!(heap.host_resource_peak_count(), 1);
    assert_eq!(Arc::strong_count(&input), 1);
    heap.collect(std::iter::empty());
    assert_eq!(bytes.used(), 0);
}

#[test]
fn unclosed_gc_and_teardown_refund_shared_tree_budgets() {
    let mut parent = Heap::new();
    parent.set_max_bytes(1_000_000);
    parent.set_max_host_resources(2);
    parent.set_host_resource_limits(HashMap::from([("json_reader".into(), 2)]));
    let mut child = Heap::new();
    child.share_tree_budgets(&parent);
    let input: Arc<str> = "null".into();
    let value = child.new_json_reader(input.clone()).unwrap();
    let bytes = parent.memory_budget().unwrap().clone();
    assert!(bytes.used() > 0);
    assert_eq!(Arc::strong_count(&input), 2);
    let Value::Heap(root) = value else { panic!() };
    child.collect(std::iter::once(root));
    assert!(bytes.used() > 0);
    child.collect(std::iter::empty());
    assert_eq!(bytes.used(), 0);
    assert_eq!(Arc::strong_count(&input), 1);
    child.new_json_reader(input.clone()).unwrap();
    drop(child);
    assert_eq!(bytes.used(), 0);
    assert_eq!(Arc::strong_count(&input), 1);
    assert_eq!(parent.host_resource_budget.as_ref().unwrap().used(), 0);
    assert_eq!(
        parent
            .host_resource_kind_budget(HostHandleKind::JsonReader)
            .unwrap()
            .used(),
        0
    );
}

#[test]
fn failed_admission_rolls_back_memory_objects_and_resource_slots() {
    for mode in 0..4 {
        let mut heap = Heap::new();
        heap.set_max_bytes(1_000_000);
        heap.set_max_host_resources(1);
        match mode {
            0 => heap.set_max_objects(0),
            1 => heap.set_max_bytes(Heap::json_reader_creation_bytes(4).unwrap() - 1),
            2 => heap.set_max_host_resources(0),
            _ => heap.set_host_resource_limits(HashMap::from([("json_reader".into(), 0)])),
        }
        let input: Arc<str> = "null".into();
        assert!(heap.new_json_reader(input.clone()).is_err(), "{mode}");
        assert_eq!(heap.live_count(), 0);
        assert_eq!(heap.current_bytes, 0);
        assert_eq!(heap.memory_budget().unwrap().used(), 0);
        assert_eq!(heap.host_resource_budget.as_ref().unwrap().used(), 0);
        assert_eq!(heap.host_resource_count(), 0);
        assert_eq!(Arc::strong_count(&input), 1);
    }
}

#[test]
fn injected_partial_construction_rolls_back_every_reserved_counter() {
    for stage in [
        AdmissionStage::Total,
        AdmissionStage::Slots,
        AdmissionStage::Owner,
    ] {
        let mut heap = Heap::new();
        heap.set_max_bytes(1_000_000);
        heap.set_max_host_resources(2);
        heap.set_host_resource_limits(HashMap::from([("json_reader".into(), 2)]));
        heap.json_reader_test_fault = Some(stage);
        let input: Arc<str> = "null".into();
        assert!(
            heap.new_json_reader(input.clone())
                .unwrap_err()
                .contains("injected")
        );
        assert_eq!(Arc::strong_count(&input), 1);
        assert_eq!(heap.memory_budget().unwrap().used(), 0);
        assert_eq!(heap.host_resource_budget.as_ref().unwrap().used(), 0);
        assert_eq!(
            heap.host_resource_kind_budget(HostHandleKind::JsonReader)
                .unwrap()
                .used(),
            0
        );
        assert_eq!(heap.current_bytes, 0);
        assert_eq!(heap.live_count(), 0);
        assert!(heap.peak_bytes >= Heap::json_reader_creation_bytes(input.len()).unwrap());
        heap.json_reader_test_fault = None;
        let r = heap.new_json_reader(input.clone()).unwrap();
        heap.close_json_reader(r).unwrap();
        heap.collect(std::iter::empty());
        assert_eq!(heap.memory_budget().unwrap().used(), 0);
    }
}

#[test]
fn lease_error_drops_owner_then_closes_aliases_and_refunds_once() {
    let mut heap = Heap::new();
    heap.set_max_bytes(1_000_000);
    heap.set_max_host_resources(1);
    let input: Arc<str> = "1.0".into();
    let value = heap.new_json_reader(input.clone()).unwrap();
    let result = heap.with_json_reader(value, |r, _| r.read_int().map_err(|e| e.to_string()));
    assert!(result.is_err());
    assert!(heap.json_reader(value).is_err());
    assert_eq!(heap.host_resource_count(), 0);
    assert_eq!(Arc::strong_count(&input), 1);
    assert_eq!(heap.current_bytes, Heap::json_handle_bytes());
    heap.close_json_reader(value).unwrap();
    heap.collect(std::iter::empty());
    assert_eq!(heap.memory_budget().unwrap().used(), 0);
}

#[test]
fn output_admission_transfers_without_double_charging_or_partial_failure() {
    let mut heap = Heap::new();
    heap.set_max_bytes(1_000_000);
    heap.set_max_objects(2);
    let (objects, bound) = Heap::json_output_bound(100, true).unwrap();
    let admission = heap.reserve_json_output(objects, bound).unwrap();
    assert_eq!(heap.memory_budget().unwrap().used(), bound as u64);
    let output = heap
        .alloc_json_output(Output::Field(Some("key")), admission)
        .unwrap();
    assert_eq!(heap.live_count(), 2);
    assert_eq!(heap.current_bytes, 2 * size_of::<HeapObject>() + 3);
    assert_eq!(
        heap.memory_budget().unwrap().used(),
        heap.current_bytes as u64
    );
    let Value::Heap(r) = output else { panic!() };
    heap.collect(std::iter::once(r));
    assert_eq!(heap.live_count(), 2);
    heap.collect(std::iter::empty());
    assert_eq!(heap.memory_budget().unwrap().used(), 0);
    let admission = heap.reserve_json_output(1, 1).unwrap();
    assert!(
        heap.alloc_json_output(Output::Text("too large"), admission)
            .is_err()
    );
    assert_eq!(heap.live_count(), 0);
    assert_eq!(heap.memory_budget().unwrap().used(), 0);
}

#[test]
fn per_kind_and_total_limits_are_shared_across_siblings() {
    let mut parent = Heap::new();
    parent.set_max_bytes(1_000_000);
    parent.set_max_host_resources(2);
    parent.set_host_resource_limits(HashMap::from([("json_reader".into(), 1)]));
    let mut a = Heap::new();
    a.share_tree_budgets(&parent);
    let mut b = Heap::new();
    b.share_tree_budgets(&parent);
    let value = a.new_json_reader("[]".into()).unwrap();
    assert!(
        b.new_json_reader("[]".into())
            .unwrap_err()
            .contains("for json_reader")
    );
    a.close_json_reader(value).unwrap();
    assert!(b.new_json_reader("[]".into()).is_ok());
}

#[test]
fn wrong_kind_dangling_and_generic_unadmitted_resources_cannot_dispatch() {
    let mut heap = Heap::new();
    let wrong = heap.alloc_host_resource(HostResource::Unsupported).unwrap();
    assert!(heap.json_reader(wrong).is_err());
    assert!(heap.close_json_reader(wrong).is_err());
    assert_eq!(heap.host_resource_count(), 1);
    let fake = Value::Heap(
        heap.alloc(HeapObject::HostHandle {
            kind: HostHandleKind::JsonReader,
            id: HostHandleId(999),
        })
        .unwrap(),
    );
    assert!(heap.json_reader(fake).is_err());
    assert!(heap.close_json_reader(fake).is_err());
    let reader = OwnedReader::new("[]".into(), Limits::default()).unwrap();
    assert!(matches!(
        heap.alloc_host_resource(HostResource::JsonReader(vec![reader])),
        Err(HeapLimitExceeded::NativeAdmissionRequired)
    ));
    assert_eq!(heap.host_resource_count(), 1);
    heap.next_host_handle_id = u64::MAX;
    assert!(
        heap.new_json_reader("[]".into())
            .unwrap_err()
            .contains("IDs exhausted")
    );
}
