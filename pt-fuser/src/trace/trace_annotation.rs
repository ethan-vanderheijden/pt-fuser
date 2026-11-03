use indexmap::IndexMap;

use crate::trace::frame::Annotation;

pub type RootAnnotations = Option<Box<IndexMap<String, Annotation>>>;

pub trait AnnotationProvider {
    fn annotations(&self) -> &RootAnnotations;
    fn annotations_mut(&mut self) -> &mut RootAnnotations;
}

pub trait KnownAnnotation {
    type Parent: KnownAnnotation<Value = IndexMap<String, Annotation>>;
    type Value: Into<Annotation>;
    const NAME: &'static str;

    fn extract(provider: &impl AnnotationProvider) -> Option<&Self::Value>
    where
        for<'a> &'a Annotation: TryInto<&'a Self::Value>,
    {
        let parent_map = Self::Parent::extract(provider)?;
        let annotation = parent_map.get(Self::NAME)?;
        annotation.try_into().ok()
    }

    fn extract_mut(provider: &mut impl AnnotationProvider) -> Option<&mut Self::Value>
    where
        for<'a> &'a mut Annotation: TryInto<&'a mut Self::Value>,
    {
        let parent_map = Self::Parent::extract_mut(provider)?;
        let annotation = parent_map.get_mut(Self::NAME)?;
        annotation.try_into().ok()
    }

    fn insert(provider: &mut impl AnnotationProvider, value: Self::Value) {
        let parent_map = match Self::Parent::extract_mut(provider) {
            // try to re-use existing map, only insert a new one if it doesn't exist
            Some(map) => map,
            None => {
                Self::Parent::insert(provider, IndexMap::new());
                Self::Parent::extract_mut(provider).unwrap()
            }
        };
        // if Self::Value is a map, merge it with any existing map instead of replacing it
        // this lets you insert into a parent annotation without deleting any previously inserted children
        match (value.into(), parent_map.get_mut(Self::NAME)) {
            (Annotation::Map(map), Some(Annotation::Map(existing_map))) => {
                for (key, value) in map {
                    existing_map.insert(key, value);
                }
            }
            (value, _) => {
                parent_map.insert(Self::NAME.to_string(), value);
            }
        };
    }
}

impl KnownAnnotation for () {
    type Parent = ();
    type Value = IndexMap<String, Annotation>;
    const NAME: &'static str = "";

    fn extract(provider: &impl AnnotationProvider) -> Option<&Self::Value> {
        provider.annotations().as_ref().map(|b| b.as_ref())
    }

    fn extract_mut(provider: &mut impl AnnotationProvider) -> Option<&mut Self::Value> {
        provider.annotations_mut().as_mut().map(|b| b.as_mut())
    }

    fn insert(provider: &mut impl AnnotationProvider, value: Self::Value) {
        provider.annotations_mut().replace(Box::new(value));
    }
}

pub struct MergingStats;

impl KnownAnnotation for MergingStats {
    type Parent = ();
    type Value = IndexMap<String, Annotation>;
    const NAME: &'static str = "Merging Stats";
}

pub struct MergingRawLatencies;

impl KnownAnnotation for MergingRawLatencies {
    type Parent = ();
    type Value = IndexMap<String, Annotation>;
    const NAME: &'static str = "Merging Raw Latencies";
}

pub struct MergingNoiseContribution;

impl KnownAnnotation for MergingNoiseContribution {
    type Parent = MergingStats;
    type Value = f64;
    const NAME: &'static str = "Noise Contribution (NC)";
}

pub struct DataRate;

impl KnownAnnotation for DataRate {
    type Parent = ();
    type Value = IndexMap<String, Annotation>;
    const NAME: &'static str = "Data Rate (MiB/s)";
}

pub struct DataRateStart;

impl KnownAnnotation for DataRateStart {
    type Parent = DataRate;
    type Value = f64;
    const NAME: &'static str = "at function's start";
}

pub struct DataRateEnd;

impl KnownAnnotation for DataRateEnd {
    type Parent = DataRate;
    type Value = f64;
    const NAME: &'static str = "at function's end";
}
