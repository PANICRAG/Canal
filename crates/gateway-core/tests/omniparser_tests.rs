// Browser module removed (CV8: replaced by canal-cv)
#![cfg(feature = "browser-legacy-tests")]

//! OmniParser tests — YOLO + OCR ONNX element detection.
//!
//! Unit tests run without model files.
//! Integration tests (marked `#[ignore]`) require models in OMNIPARSER_MODEL_DIR.

#[cfg(feature = "omniparser")]
mod omniparser_unit_tests {
    use gateway_core::browser::omniparser::{
        find_all_by_text, find_by_text, to_css_coords, to_normalized, DetectedElement, ElementType,
    };
    use gateway_core::browser::omniparser_yolo::{
        center_to_corner, compute_iou, compute_letterbox_params, non_max_suppression,
        unpad_coordinates, YoloDetection,
    };

    // ============================================
    // IoU Tests
    // ============================================

    #[test]
    fn test_compute_iou_identical_boxes() {
        let a = [10.0, 10.0, 50.0, 50.0];
        let b = [10.0, 10.0, 50.0, 50.0];
        let iou = compute_iou(&a, &b);
        assert!(
            (iou - 1.0).abs() < 1e-5,
            "Identical boxes should have IoU=1.0, got {}",
            iou
        );
    }

    #[test]
    fn test_compute_iou_no_overlap() {
        let a = [0.0, 0.0, 10.0, 10.0];
        let b = [20.0, 20.0, 30.0, 30.0];
        let iou = compute_iou(&a, &b);
        assert!(
            iou.abs() < 1e-5,
            "Non-overlapping boxes should have IoU=0.0, got {}",
            iou
        );
    }

    #[test]
    fn test_compute_iou_partial_overlap() {
        let a = [0.0, 0.0, 10.0, 10.0]; // area=100
        let b = [5.0, 0.0, 15.0, 10.0]; // area=100
                                        // intersection = 5*10=50, union = 100+100-50=150
        let iou = compute_iou(&a, &b);
        let expected = 50.0 / 150.0;
        assert!(
            (iou - expected).abs() < 1e-5,
            "Expected IoU ~{:.4}, got {}",
            expected,
            iou
        );
    }

    #[test]
    fn test_compute_iou_contained_box() {
        let a = [0.0, 0.0, 100.0, 100.0]; // area=10000
        let b = [25.0, 25.0, 75.0, 75.0]; // area=2500
                                          // intersection = 50*50=2500, union = 10000+2500-2500=10000
        let iou = compute_iou(&a, &b);
        let expected = 2500.0 / 10000.0;
        assert!(
            (iou - expected).abs() < 1e-5,
            "Expected IoU ~{:.4}, got {}",
            expected,
            iou
        );
    }

    #[test]
    fn test_compute_iou_zero_area() {
        let a = [0.0, 0.0, 0.0, 0.0]; // zero area
        let b = [0.0, 0.0, 10.0, 10.0];
        let iou = compute_iou(&a, &b);
        assert!(iou.abs() < 1e-5, "Zero area box should have IoU=0.0");
    }

    // ============================================
    // NMS Tests
    // ============================================

    #[test]
    fn test_nms_removes_overlapping() {
        let mut dets = vec![
            YoloDetection {
                bbox: [10.0, 10.0, 50.0, 50.0],
                confidence: 0.9,
                class_id: 0,
            },
            YoloDetection {
                bbox: [12.0, 12.0, 52.0, 52.0],
                confidence: 0.7,
                class_id: 0,
            },
            YoloDetection {
                bbox: [100.0, 100.0, 150.0, 150.0],
                confidence: 0.8,
                class_id: 0,
            },
        ];
        let result = non_max_suppression(&mut dets, 0.45);
        assert_eq!(
            result.len(),
            2,
            "NMS should keep 2 boxes (suppress overlapping lower-conf)"
        );
        assert!((result[0].confidence - 0.9).abs() < 1e-5);
        assert!((result[1].confidence - 0.8).abs() < 1e-5);
    }

    #[test]
    fn test_nms_keeps_non_overlapping() {
        let mut dets = vec![
            YoloDetection {
                bbox: [0.0, 0.0, 10.0, 10.0],
                confidence: 0.5,
                class_id: 0,
            },
            YoloDetection {
                bbox: [50.0, 50.0, 60.0, 60.0],
                confidence: 0.6,
                class_id: 0,
            },
            YoloDetection {
                bbox: [100.0, 100.0, 110.0, 110.0],
                confidence: 0.7,
                class_id: 0,
            },
        ];
        let result = non_max_suppression(&mut dets, 0.45);
        assert_eq!(
            result.len(),
            3,
            "NMS should keep all 3 non-overlapping boxes"
        );
    }

    #[test]
    fn test_nms_empty_input() {
        let mut dets: Vec<YoloDetection> = Vec::new();
        let result = non_max_suppression(&mut dets, 0.45);
        assert!(result.is_empty());
    }

    #[test]
    fn test_nms_single_detection() {
        let mut dets = vec![YoloDetection {
            bbox: [10.0, 10.0, 50.0, 50.0],
            confidence: 0.9,
            class_id: 0,
        }];
        let result = non_max_suppression(&mut dets, 0.45);
        assert_eq!(result.len(), 1);
    }

    // ============================================
    // Coordinate Conversion Tests
    // ============================================

    #[test]
    fn test_center_to_corner_conversion() {
        let (x1, y1, x2, y2) = center_to_corner(50.0, 50.0, 20.0, 30.0);
        assert!((x1 - 40.0).abs() < 1e-5);
        assert!((y1 - 35.0).abs() < 1e-5);
        assert!((x2 - 60.0).abs() < 1e-5);
        assert!((y2 - 65.0).abs() < 1e-5);
    }

    #[test]
    fn test_center_to_corner_zero_size() {
        let (x1, y1, x2, y2) = center_to_corner(50.0, 50.0, 0.0, 0.0);
        assert!((x1 - 50.0).abs() < 1e-5);
        assert!((y1 - 50.0).abs() < 1e-5);
        assert!((x2 - 50.0).abs() < 1e-5);
        assert!((y2 - 50.0).abs() < 1e-5);
    }

    #[test]
    fn test_letterbox_coordinate_roundtrip() {
        let (scale, pad_top, pad_left) = compute_letterbox_params(4070, 2216, 1280);

        // Original image coordinate
        let orig_x = 2035.0f32;
        let orig_y = 1108.0f32;

        // Forward: original → letterbox
        let lb_x = orig_x * scale + pad_left;
        let lb_y = orig_y * scale + pad_top;

        // Reverse: letterbox → original
        let (recovered_x, recovered_y) = unpad_coordinates(lb_x, lb_y, scale, pad_top, pad_left);

        assert!(
            (recovered_x - orig_x).abs() < 1.0,
            "X roundtrip: expected {}, got {}",
            orig_x,
            recovered_x
        );
        assert!(
            (recovered_y - orig_y).abs() < 1.0,
            "Y roundtrip: expected {}, got {}",
            orig_y,
            recovered_y
        );
    }

    #[test]
    fn test_letterbox_params_wider_image() {
        let (scale, pad_top, pad_left) = compute_letterbox_params(1920, 1080, 1280);
        // scale = min(1280/1920, 1280/1080) = 0.667
        assert!((scale - 1280.0 / 1920.0).abs() < 0.01);
        assert!(
            pad_left.abs() < 1.0,
            "Wide image should have no left padding"
        );
        assert!(
            pad_top > 100.0,
            "Wide image should have vertical padding, got {}",
            pad_top
        );
    }

    #[test]
    fn test_letterbox_params_taller_image() {
        let (scale, pad_top, pad_left) = compute_letterbox_params(1080, 1920, 1280);
        // scale = min(1280/1080, 1280/1920) = 0.667
        assert!((scale - 1280.0 / 1920.0).abs() < 0.01);
        assert!(
            pad_left > 100.0,
            "Tall image should have horizontal padding, got {}",
            pad_left
        );
        assert!(pad_top.abs() < 1.0, "Tall image should have no top padding");
    }

    #[test]
    fn test_letterbox_params_square_image() {
        let (scale, pad_top, pad_left) = compute_letterbox_params(1280, 1280, 1280);
        assert!((scale - 1.0).abs() < 0.01);
        assert!(pad_top.abs() < 1.0);
        assert!(pad_left.abs() < 1.0);
    }

    // ============================================
    // CSS Coordinate Tests
    // ============================================

    #[test]
    fn test_dpr_coordinate_conversion() {
        // 4070×2216 image, 2035×1108 viewport (DPR ≈ 2)
        let (css_x, css_y) = to_css_coords(306, 198, 4070, 2216, 2035, 1108);
        assert!(
            (css_x as i32 - 153).abs() <= 1,
            "Expected CSS X ~153, got {}",
            css_x
        );
        assert!(
            (css_y as i32 - 99).abs() <= 1,
            "Expected CSS Y ~99, got {}",
            css_y
        );
    }

    #[test]
    fn test_css_coords_1to1_mapping() {
        // Non-retina: image == viewport
        let (css_x, css_y) = to_css_coords(500, 300, 1920, 1080, 1920, 1080);
        assert_eq!(css_x, 500);
        assert_eq!(css_y, 300);
    }

    #[test]
    fn test_css_coords_within_viewport() {
        let (css_x, css_y) = to_css_coords(4000, 2200, 4070, 2216, 2035, 1108);
        assert!(css_x <= 2035, "CSS X {} exceeds viewport width 2035", css_x);
        assert!(
            css_y <= 1108,
            "CSS Y {} exceeds viewport height 1108",
            css_y
        );
    }

    #[test]
    fn test_css_coords_zero_dimensions() {
        let (x, y) = to_css_coords(100, 200, 0, 0, 1920, 1080);
        assert_eq!((x, y), (0, 0));
    }

    // ============================================
    // Normalized Coordinate Tests
    // ============================================

    #[test]
    fn test_normalized_coords() {
        let norm = to_normalized(100.0, 200.0, 300.0, 400.0, 1000, 1000);
        assert!((norm[0] - 0.1).abs() < 1e-5);
        assert!((norm[1] - 0.2).abs() < 1e-5);
        assert!((norm[2] - 0.3).abs() < 1e-5);
        assert!((norm[3] - 0.4).abs() < 1e-5);
    }

    #[test]
    fn test_normalized_coords_clamped() {
        let norm = to_normalized(-10.0, -20.0, 1100.0, 1200.0, 1000, 1000);
        assert_eq!(norm[0], 0.0);
        assert_eq!(norm[1], 0.0);
        assert_eq!(norm[2], 1.0);
        assert_eq!(norm[3], 1.0);
    }

    #[test]
    fn test_normalized_coords_zero_image() {
        let norm = to_normalized(10.0, 20.0, 30.0, 40.0, 0, 0);
        assert_eq!(norm, [0.0, 0.0, 0.0, 0.0]);
    }

    // ============================================
    // Text Matching Tests
    // ============================================

    fn make_element(id: u32, text: &str, conf: f32) -> DetectedElement {
        DetectedElement {
            id,
            element_type: ElementType::Text,
            bbox_px: [0, 0, 100, 30],
            bbox_norm: [0.0, 0.0, 0.1, 0.01],
            center_px: [50, 15],
            center_css: [25, 8],
            conf,
            text: text.to_string(),
            width: 100,
            height: 30,
        }
    }

    #[test]
    fn test_find_by_text_exact_match() {
        let elements = vec![
            make_element(0, "写邮件", 0.95),
            make_element(1, "收件人", 0.90),
            make_element(2, "主题", 0.88),
            make_element(3, "发送", 0.85),
        ];
        let found = find_by_text(&elements, "收件人", false, 0.8);
        assert!(found.is_some(), "Should find exact match");
        assert_eq!(found.unwrap().id, 1);
    }

    #[test]
    fn test_find_by_text_case_insensitive() {
        let elements = vec![
            make_element(0, "Submit", 0.9),
            make_element(1, "Cancel", 0.85),
        ];
        let found = find_by_text(&elements, "submit", false, 0.8);
        assert!(found.is_some());
        assert_eq!(found.unwrap().id, 0);
    }

    #[test]
    fn test_find_by_text_substring_match() {
        let elements = vec![
            make_element(0, "写邮件按钮", 0.95),
            make_element(1, "收件人输入框", 0.90),
        ];
        let found = find_by_text(&elements, "写邮件", false, 0.8);
        assert!(found.is_some());
        assert_eq!(found.unwrap().id, 0);
    }

    #[test]
    fn test_find_by_text_fuzzy_match() {
        let elements = vec![
            make_element(0, "Submitt", 0.9), // typo
            make_element(1, "Cancel", 0.85),
        ];
        let found = find_by_text(&elements, "Submit", true, 0.8);
        assert!(found.is_some(), "Fuzzy should match 'Submitt' → 'Submit'");
        assert_eq!(found.unwrap().id, 0);
    }

    #[test]
    fn test_find_by_text_fuzzy_disabled() {
        // "Sbumt" is similar to "Submit" but not a substring, so it should
        // only match with fuzzy enabled.
        let elements = vec![
            make_element(0, "Sbumt", 0.9),
            make_element(1, "Cancel", 0.85),
        ];
        let found = find_by_text(&elements, "Submit", false, 0.8);
        assert!(
            found.is_none(),
            "Without fuzzy, 'Sbumt' should not match 'Submit'"
        );
    }

    #[test]
    fn test_fuzzy_match_disambiguation() {
        let elements = vec![
            make_element(0, "阿里云发送设置", 0.95),
            make_element(1, "发送", 0.90),
        ];
        // "发送" should prefer exact match over substring
        let found = find_by_text(&elements, "发送", true, 0.8);
        assert!(found.is_some());
        assert_eq!(
            found.unwrap().id,
            1,
            "Exact match '发送' should beat substring '阿里云发送设置'"
        );
    }

    #[test]
    fn test_find_by_text_not_found() {
        let elements = vec![
            make_element(0, "写邮件", 0.95),
            make_element(1, "收件人", 0.90),
        ];
        let found = find_by_text(&elements, "不存在的文字", false, 0.8);
        assert!(found.is_none());
    }

    #[test]
    fn test_find_by_text_empty_elements() {
        let elements: Vec<DetectedElement> = Vec::new();
        let found = find_by_text(&elements, "anything", true, 0.8);
        assert!(found.is_none());
    }

    #[test]
    fn test_find_all_by_text_multiple_matches() {
        let elements = vec![
            make_element(0, "发送邮件", 0.95),
            make_element(1, "发送", 0.90),
            make_element(2, "重新发送", 0.85),
            make_element(3, "取消", 0.80),
        ];
        let results = find_all_by_text(&elements, "发送", true, 0.8, 5);
        assert!(
            results.len() >= 2,
            "Should find multiple matches for '发送'"
        );
    }

    // ============================================
    // Invalid Input Tests
    // ============================================

    #[test]
    fn test_invalid_image_handling() {
        // 1x1 image should not panic
        let img = image::DynamicImage::new_rgb8(1, 1);
        // We can't test detect() without a model, but we can verify the image is valid
        assert_eq!(img.width(), 1);
        assert_eq!(img.height(), 1);
    }

    #[test]
    fn test_missing_models_yolo() {
        use gateway_core::browser::omniparser_yolo::YoloDetector;
        let result = YoloDetector::new("/nonexistent/model.onnx", 1280, 0.05, 0.45);
        assert!(result.is_err(), "Should fail with missing model file");
    }

    #[test]
    fn test_missing_models_ocr() {
        use gateway_core::browser::omniparser_ocr::OcrDetector;
        let result = OcrDetector::new(
            "/nonexistent/det.onnx",
            "/nonexistent/rec.onnx",
            "/nonexistent/dict.txt",
        );
        assert!(result.is_err(), "Should fail with missing model files");
    }

    #[test]
    fn test_missing_models_omniparser() {
        use gateway_core::browser::omniparser::{
            MatchingConfig, OcrConfig, OmniParser, OmniParserConfig, YoloConfig,
        };
        let config = OmniParserConfig {
            model_dir: "/nonexistent/models".to_string(),
            yolo: YoloConfig {
                input_size: 1280,
                confidence_threshold: 0.05,
                iou_threshold: 0.45,
            },
            ocr: OcrConfig {
                text_threshold: 0.7,
            },
            matching: MatchingConfig {
                fuzzy_threshold: 0.8,
                max_results: 200,
            },
        };
        let result = OmniParser::new(config);
        assert!(
            result.is_err(),
            "Should fail gracefully with missing model directory"
        );
    }
}

// ============================================
// Integration Tests (require model files)
// ============================================

#[cfg(feature = "omniparser")]
mod omniparser_integration_tests {
    use gateway_core::browser::omniparser::{
        MatchingConfig, OcrConfig, OmniParser, OmniParserConfig, YoloConfig,
    };

    fn get_model_dir() -> String {
        std::env::var("OMNIPARSER_MODEL_DIR").unwrap_or_else(|_| "models/omniparser".to_string())
    }

    fn models_available() -> bool {
        let dir = get_model_dir();
        std::path::Path::new(&format!("{}/icon_detect.onnx", dir)).exists()
    }

    fn create_parser() -> OmniParser {
        let config = OmniParserConfig {
            model_dir: get_model_dir(),
            yolo: YoloConfig {
                input_size: 1280,
                confidence_threshold: 0.05,
                iou_threshold: 0.45,
            },
            ocr: OcrConfig {
                text_threshold: 0.7,
            },
            matching: MatchingConfig {
                fuzzy_threshold: 0.8,
                max_results: 200,
            },
        };
        OmniParser::new(config).expect("Failed to create OmniParser")
    }

    #[test]
    #[ignore = "Requires ONNX model files"]
    fn test_yolo_model_loading() {
        if !models_available() {
            eprintln!("Skipping: models not found at {}", get_model_dir());
            return;
        }
        use gateway_core::browser::omniparser_yolo::YoloDetector;
        let dir = get_model_dir();
        let result = YoloDetector::new(&format!("{}/icon_detect.onnx", dir), 1280, 0.05, 0.45);
        assert!(result.is_ok(), "YOLO model should load: {:?}", result.err());
    }

    #[test]
    #[ignore = "Requires ONNX model files"]
    fn test_full_detection_pipeline() {
        if !models_available() {
            eprintln!("Skipping: models not found at {}", get_model_dir());
            return;
        }

        let mut parser = create_parser();

        // Create a simple test image (1920x1080)
        let img = image::DynamicImage::new_rgb8(1920, 1080);
        let result = parser.detect(&img, 1920, 1080);
        assert!(result.is_ok(), "Detection should succeed on blank image");

        let result = result.unwrap();
        assert!(result.success);
        // Blank image may have 0 detections, which is fine
        println!(
            "Blank image: {} elements, timing: {:?}",
            result.elements.len(),
            result.timing
        );
    }

    #[test]
    #[ignore = "Requires ONNX model files"]
    fn test_first_inference_latency() {
        if !models_available() {
            eprintln!("Skipping: models not found at {}", get_model_dir());
            return;
        }

        let mut parser = create_parser();
        let img = image::DynamicImage::new_rgb8(1920, 1080);

        let start = std::time::Instant::now();
        let _ = parser.detect(&img, 1920, 1080);
        let first_ms = start.elapsed().as_millis();

        let start = std::time::Instant::now();
        let _ = parser.detect(&img, 1920, 1080);
        let second_ms = start.elapsed().as_millis();

        println!("First inference: {}ms, Second: {}ms", first_ms, second_ms);
        assert!(
            first_ms < 60_000,
            "First inference should complete within 60s, took {}ms",
            first_ms
        );
        assert!(
            second_ms < 30_000,
            "Steady-state inference should be <30s, took {}ms",
            second_ms
        );
    }

    #[test]
    #[ignore = "Requires ONNX model files"]
    fn test_concurrent_detection() {
        if !models_available() {
            eprintln!("Skipping: models not found at {}", get_model_dir());
            return;
        }

        let parser = std::sync::Arc::new(std::sync::Mutex::new(create_parser()));
        let img = std::sync::Arc::new(image::DynamicImage::new_rgb8(800, 600));

        let handles: Vec<_> = (0..3)
            .map(|i| {
                let p = parser.clone();
                let im = img.clone();
                std::thread::spawn(move || {
                    let mut parser = p.lock().unwrap();
                    let result = parser.detect(&im, 800, 600);
                    println!("Thread {}: {:?}", i, result.is_ok());
                    result.is_ok()
                })
            })
            .collect();

        for (i, h) in handles.into_iter().enumerate() {
            let ok = h.join().expect("Thread panicked");
            assert!(ok, "Concurrent detection {} should succeed", i);
        }
    }
}
