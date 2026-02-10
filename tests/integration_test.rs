//! Integration tests for hyperforge hub

use futures::StreamExt;
use plexus_core::plexus::DynamicHub;
use hyperforge::{HyperforgeEvent, HyperforgeHub};
use std::sync::Arc;

#[tokio::test]
async fn test_hyperforge_as_plugin() {
    // Create hyperforge activation
    let hyperforge = HyperforgeHub::new();

    // Register in DynamicHub
    let hub = Arc::new(DynamicHub::new("testhub").register(hyperforge));

    // Call hyperforge.status via DynamicHub routing
    let mut stream = hub.route("hyperforge.status", serde_json::json!({})).await.unwrap();

    let mut found_status = false;
    while let Some(item) = stream.next().await {
        if let plexus_core::plexus::PlexusStreamItem::Data { content, .. } = item {
            if let Ok(event) = serde_json::from_value::<HyperforgeEvent>(content) {
                match event {
                    HyperforgeEvent::Status { version, description } => {
                        assert_eq!(version, "3.0.0");
                        assert!(description.contains("LFORGE2"));
                        found_status = true;
                    }
                    _ => {}
                }
            }
        }
    }

    assert!(found_status, "Should have received status event");
}

#[tokio::test]
async fn test_hyperforge_version_method() {
    // Create hyperforge activation
    let hyperforge = HyperforgeHub::new();

    // Register in DynamicHub
    let hub = Arc::new(DynamicHub::new("testhub").register(hyperforge));

    // Call hyperforge.version via DynamicHub routing
    let mut stream = hub.route("hyperforge.version", serde_json::json!({})).await.unwrap();

    let mut found_version = false;
    while let Some(item) = stream.next().await {
        if let plexus_core::plexus::PlexusStreamItem::Data { content, .. } = item {
            if let Ok(event) = serde_json::from_value::<HyperforgeEvent>(content) {
                match event {
                    HyperforgeEvent::Info { message } => {
                        assert!(message.contains("3.0.0"));
                        assert!(message.contains("LFORGE2"));
                        found_version = true;
                    }
                    _ => {}
                }
            }
        }
    }

    assert!(found_version, "Should have received version info event");
}

#[tokio::test]
async fn test_dynamic_hub_lists_hyperforge() {
    // Create hyperforge activation
    let hyperforge = HyperforgeHub::new();

    // Register in DynamicHub
    let hub = DynamicHub::new("testhub").register(hyperforge);

    // Check that hyperforge is listed in activations
    let activations = hub.list_activations_info();
    let hyperforge_activation = activations
        .iter()
        .find(|a| a.namespace == "hyperforge")
        .expect("hyperforge should be listed in activations");

    assert_eq!(hyperforge_activation.version, "3.0.0");
    assert_eq!(hyperforge_activation.description, "Multi-forge repository management");

    // Check that methods are listed
    assert!(hyperforge_activation.methods.contains(&"status".to_string()));
    assert!(hyperforge_activation.methods.contains(&"version".to_string()));
}
